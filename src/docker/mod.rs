//! 選んだ colima インスタンスの Docker API への接続と、一覧の取得。

pub mod actions;
mod df;
mod logs;

use crate::i18n::{Lang, Msg};
use crate::snapshot::{ContainerRow, DockerSummary, ImageRow, NetworkRow, VolumeRow};
use bollard::models::{ContainerSummary, PortSummary, Volume};
use bollard::query_parameters::{
    EventsOptionsBuilder, ListContainersOptionsBuilder, ListImagesOptions, ListNetworksOptions,
    ListVolumesOptions,
};
use bollard::{API_DEFAULT_VERSION, Docker};
use futures_util::{Stream, StreamExt};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

const REQUEST_TIMEOUT_SECS: u64 = 120;

/// 一覧や失敗の判定が変わる action だけを購読する。`exec_*` や `health_status` は一覧に効かず、
/// `docker exec` や healthcheck のたびに取り直しが走る。`event` フィルタは型をまたいで効くので、
/// イメージ・ボリューム・ネットワークの変化（pull、delete、connect など）もここに入れる。
const EVENT_ACTIONS: [&str; 22] = [
    "create",
    "destroy",
    "die",
    "start",
    "stop",
    "kill",
    "pause",
    "unpause",
    "rename",
    "oom",
    "restart",
    "update",
    "commit",
    "pull",
    "delete",
    "tag",
    "untag",
    "import",
    "load",
    "remove",
    "connect",
    "disconnect",
];

/// Docker 操作の失敗。文言は `message` で言語を選んで作る。
#[derive(Debug, Clone, PartialEq)]
pub enum DockerError {
    Connect(String),
    /// bollard が返した本文。表示用の文言の後ろにそのまま付ける。
    Api(String),
    InvalidPort(String),
    InvalidEnv(String),
    InvalidMount(String),
    /// 名前・ID・イメージ参照が、URL の一部として安全な形ではない。
    InvalidId(String),
    VolumeNameTooLong,
}

impl DockerError {
    pub fn message(&self, lang: Lang) -> String {
        let t = |m: Msg| m.text(lang);
        match self {
            Self::Connect(e) => format!("{}: {e}", t(Msg::DockerConnect)),
            Self::Api(e) => format!("{}: {e}", t(Msg::DockerApi)),
            Self::InvalidPort(s) => format!("{}: {s}", t(Msg::DockerPort)),
            Self::InvalidEnv(s) => format!("{}: {s:?}", t(Msg::DockerEnv)),
            Self::InvalidMount(s) => format!("{}: {s:?}", t(Msg::DockerMount)),
            Self::InvalidId(s) => format!("{}: {s:?}", t(Msg::DockerInvalidId)),
            Self::VolumeNameTooLong => t(Msg::DockerVolumeNameTooLong)
                .replace("{max}", &actions::VOLUME_NAME_MAX.to_string()),
        }
    }
}

impl std::fmt::Display for DockerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message(Lang::En))
    }
}

impl std::error::Error for DockerError {}

impl From<bollard::errors::Error> for DockerError {
    fn from(e: bollard::errors::Error) -> Self {
        Self::Api(e.to_string())
    }
}

/// コンテナとボリュームの名前か ID、ネットワークの ID。
/// ネットワークは `my net` や `x/y` のようにこの形に合わない名前でも作れるので、名前ではなく ID で渡す。
/// bollard は名前を `/containers/{id}/...` などに埋め込むので、`/` や `?` を含む値は別の経路に当たる。
/// 受け付ける形は moby の `daemon/names` の `RestrictedNamePattern`（`^[a-zA-Z0-9][a-zA-Z0-9_.-]+$`）と同じ。
pub(crate) fn check_name(id: &str) -> Result<(), DockerError> {
    let mut chars = id.chars();
    let valid = id.len() >= 2
        && chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if valid {
        Ok(())
    } else {
        Err(DockerError::InvalidId(id.to_string()))
    }
}

/// 文法上いちばん長い参照の字数。名前（255）、`:` とタグ（1 + 128）、`@` と sha512 の digest（1 + 7 + 128）。
const IMAGE_REF_MAX: usize = NAME_TOTAL_LENGTH_MAX + 1 + 128 + 1 + 7 + 128;

/// イメージの名前・タグ・ID・digest を受ける。`/`・`:`・`@` は参照の文法（distribution/reference の
/// `regexp.go`）で正当なので、文字集合をその範囲に絞り、`.` と `..` と空のパス要素と先頭の `-` を拒む。
pub(crate) fn check_image_ref(r: &str) -> Result<(), DockerError> {
    let valid = r.len() <= IMAGE_REF_MAX
        && !r.starts_with('-')
        && r.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | ':' | '@' | '+')
        })
        && r.split('/').all(|seg| !matches!(seg, "" | "." | ".."));
    if valid {
        Ok(())
    } else {
        Err(DockerError::InvalidId(r.to_string()))
    }
}

/// `docker tag` のリポジトリ名。distribution/reference の `reference.go` の文法
/// `name := [domain '/'] remote-name` に従う。パスの部分は小文字の英数字を `.`、`_`、`__`、`-` の並びでつなぐ。
pub(crate) fn check_repo(r: &str) -> Result<(), DockerError> {
    let parts: Vec<&str> = r.split('/').collect();
    let remote = |p: &[&str]| !p.is_empty() && p.iter().all(|c| is_path_component(c));
    let valid = r.len() <= NAME_TOTAL_LENGTH_MAX
        && (remote(&parts) || (is_domain(parts[0]) && remote(&parts[1..])));
    if valid {
        Ok(())
    } else {
        Err(DockerError::InvalidId(r.to_string()))
    }
}

/// distribution/reference の `NameTotalLengthMax`。
const NAME_TOTAL_LENGTH_MAX: usize = 255;

/// `alpha-numeric [separator alpha-numeric]*`。`separator := [_.] | __ | [-]+`。
fn is_path_component(c: &str) -> bool {
    let alnum = |ch: char| ch.is_ascii_lowercase() || ch.is_ascii_digit();
    let mut rest = c;
    loop {
        let run = rest.len() - rest.trim_start_matches(alnum).len();
        if run == 0 {
            return false;
        }
        rest = &rest[run..];
        if rest.is_empty() {
            return true;
        }
        let sep = rest.len() - rest.trim_start_matches(['.', '_', '-']).len();
        let ok = match &rest[..sep] {
            "." | "_" | "__" => true,
            s => !s.is_empty() && s.chars().all(|ch| ch == '-'),
        };
        if !ok {
            return false;
        }
        rest = &rest[sep..];
    }
}

/// `host [':' port-number]`。host は `.` でつないだ英数字とハイフンの部分か、`[` と `]` で囲んだ IPv6。
fn is_domain(d: &str) -> bool {
    let (host, port) = match d.rsplit_once(':') {
        Some((h, p)) if !h.starts_with('[') || h.ends_with(']') => (h, Some(p)),
        _ => (d, None),
    };
    if port.is_some_and(|p| p.is_empty() || !p.chars().all(|ch| ch.is_ascii_digit())) {
        return false;
    }
    if let Some(v6) = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
        return !v6.is_empty() && v6.chars().all(|ch| ch.is_ascii_hexdigit() || ch == ':');
    }
    host.split('.').all(|part| {
        !part.is_empty()
            && !part.starts_with('-')
            && !part.ends_with('-')
            && part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    })
}

/// `docker tag` のタグ。distribution/reference の `tag := [\w][\w.-]{0,127}`。
pub(crate) fn check_tag(t: &str) -> Result<(), DockerError> {
    let word = |ch: char| ch.is_ascii_alphanumeric() || ch == '_';
    let mut chars = t.chars();
    let valid = t.len() <= 128
        && chars.next().is_some_and(word)
        && chars.all(|ch| word(ch) || matches!(ch, '.' | '-'));
    if valid {
        Ok(())
    } else {
        Err(DockerError::InvalidId(t.to_string()))
    }
}

#[derive(Clone)]
pub struct Engine {
    pub(crate) docker: Docker,
}

impl Engine {
    pub fn connect(socket: &Path) -> Result<Engine, DockerError> {
        let path = socket
            .to_str()
            .ok_or_else(|| DockerError::Connect(socket.display().to_string()))?;
        let docker = Docker::connect_with_unix(path, REQUEST_TIMEOUT_SECS, API_DEFAULT_VERSION)
            .map_err(|e| DockerError::Connect(e.to_string()))?;
        Ok(Engine { docker })
    }

    pub async fn summary(&self) -> Result<DockerSummary, DockerError> {
        let list_opts = ListContainersOptionsBuilder::new().all(true).build();
        let (containers, images, volumes, networks, df) = tokio::join!(
            self.docker.list_containers(Some(list_opts)),
            self.docker.list_images(None::<ListImagesOptions>),
            self.docker.list_volumes(None::<ListVolumesOptions>),
            self.docker.list_networks(None::<ListNetworksOptions>),
            // df が取れなくても（遅くて打ち切った場合も）一覧は出す。サイズだけ不明にする。
            df::fetch(&self.docker, df::DF_TIMEOUT),
        );
        let (disk, volume_usage) = df.unwrap_or_default();
        Ok(DockerSummary {
            containers: self.container_rows(containers?).await,
            images: images?
                .into_iter()
                .map(|i| ImageRow {
                    id: i.id,
                    repo_tags: i.repo_tags,
                    size_bytes: i.size.max(0) as u64,
                    created: i.created,
                })
                .collect(),
            volumes: volumes?
                .volumes
                .unwrap_or_default()
                .into_iter()
                .map(|v| {
                    let usage = volume_usage.get(&v.name).copied().unwrap_or_default();
                    volume_row(v, usage)
                })
                .collect(),
            networks: networks?
                .into_iter()
                .map(|n| {
                    let name = n.name.unwrap_or_default();
                    NetworkRow {
                        builtin: is_builtin_network(&name),
                        id: n.id.unwrap_or_default(),
                        driver: n.driver.unwrap_or_default(),
                        name,
                    }
                })
                .collect(),
            disk,
        })
    }

    async fn container_rows(&self, list: Vec<ContainerSummary>) -> Vec<ContainerRow> {
        futures_util::future::join_all(list.into_iter().map(|c| async move {
            let id = c.id.clone().unwrap_or_default();
            let state = c.state.map(|s| s.to_string()).unwrap_or_default();
            let failed = if needs_inspect(&state) {
                self.failed_by_inspect(&id, &state).await
            } else {
                false
            };
            ContainerRow {
                name: container_name(c.names.as_deref().unwrap_or_default()),
                image: c.image.unwrap_or_default(),
                image_id: c.image_id.unwrap_or_default(),
                status: c.status.unwrap_or_default(),
                ports: format_ports(c.ports.as_deref().unwrap_or_default()),
                id,
                state,
                failed,
            }
        }))
        .await
    }

    async fn failed_by_inspect(&self, id: &str, state: &str) -> bool {
        if check_name(id).is_err() {
            return is_failed(state, None, "", false);
        }
        // 消えたコンテナなど inspect に失敗したときは、失敗とは決めつけない。
        match self.docker.inspect_container(id, None).await {
            Ok(r) => {
                let s = r.state.unwrap_or_default();
                is_failed(
                    state,
                    s.exit_code,
                    s.error.as_deref().unwrap_or_default(),
                    s.oom_killed.unwrap_or(false),
                )
            }
            Err(_) => is_failed(state, None, "", false),
        }
    }

    /// 変化の通知だけを流す。中身は読み直しで取る。
    pub fn events(&self) -> impl Stream<Item = Result<(), DockerError>> + '_ {
        let filters = HashMap::from([
            ("type", vec!["container", "image", "volume", "network"]),
            ("event", EVENT_ACTIONS.to_vec()),
        ]);
        let opts = EventsOptionsBuilder::new().filters(&filters).build();
        self.docker
            .events(Some(opts))
            .map(|r| r.map(|_| ()).map_err(DockerError::from))
    }
}

/// 一覧の状態が止まっている・再起動中・作成のみのときに、失敗かどうかを inspect で調べる。
/// 起動に失敗したコンテナは `created` のまま `State.Error` を持つ。
fn needs_inspect(state: &str) -> bool {
    !matches!(state, "running" | "paused")
}

fn container_name(names: &[String]) -> String {
    names
        .first()
        .map(|n| n.trim_start_matches('/').to_string())
        .unwrap_or_default()
}

pub fn is_failed(status: &str, exit_code: Option<i64>, error: &str, oom_killed: bool) -> bool {
    match status {
        "restarting" | "dead" => true,
        _ if !error.is_empty() || oom_killed => true,
        // 137 と 143 は stop や kill による終了で、失敗ではない。
        "exited" => exit_code.is_some_and(|c| !matches!(c, 0 | 137 | 143)),
        _ => false,
    }
}

/// IPv4 と IPv6 で同じ割り当てが並ぶので、重ねて 1 つにする。
pub fn format_ports(ports: &[PortSummary]) -> String {
    let set: BTreeSet<(u16, Option<u16>, String)> = ports
        .iter()
        .map(|p| {
            let proto = p.typ.map_or("tcp".into(), |t| t.to_string());
            (p.private_port, p.public_port, proto)
        })
        .collect();
    set.into_iter()
        .map(|(private, public, proto)| match public {
            Some(host) => format!("{host}->{private}/{proto}"),
            None => format!("{private}/{proto}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Dockerが無名ボリュームに付けるラベル。名前の形でなくこれで見分ける。
const ANONYMOUS_VOLUME_LABEL: &str = "com.docker.volume.anonymous";

fn volume_row(v: Volume, usage: df::VolumeUse) -> VolumeRow {
    VolumeRow {
        prunable: usage.unused && v.labels.contains_key(ANONYMOUS_VOLUME_LABEL),
        size_bytes: usage.size,
        name: v.name,
        driver: v.driver,
        mountpoint: v.mountpoint,
    }
}

pub fn is_builtin_network(name: &str) -> bool {
    matches!(name, "bridge" | "host" | "none")
}

/// system df の `-1` は「計算していない」を表す。
pub(crate) fn size_to_option(size: i64) -> Option<u64> {
    u64::try_from(size).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::PortSummaryTypeEnum;
    use std::time::Duration;

    fn port(ip: &str, private: u16, public: Option<u16>, t: PortSummaryTypeEnum) -> PortSummary {
        PortSummary {
            ip: Some(ip.into()),
            private_port: private,
            public_port: public,
            typ: Some(t),
        }
    }

    #[test]
    fn format_ports_sorted_and_deduped() {
        let tcp = PortSummaryTypeEnum::TCP;
        let ports = [
            port("::", 80, Some(8080), tcp),
            port("0.0.0.0", 80, Some(8080), tcp),
            port("0.0.0.0", 53, Some(53), PortSummaryTypeEnum::UDP),
            port("", 443, None, tcp),
        ];
        assert_eq!(format_ports(&ports), "53->53/udp, 8080->80/tcp, 443/tcp");
        assert_eq!(format_ports(&[]), "");
    }

    #[test]
    fn builtin_networks_are_protected() {
        for n in ["bridge", "host", "none"] {
            assert!(is_builtin_network(n), "{n}");
        }
        for n in ["my-net", "", "Bridge", "bridge2"] {
            assert!(!is_builtin_network(n), "{n}");
        }
    }

    #[test]
    fn size_minus_one_becomes_none() {
        assert_eq!(size_to_option(-1), None);
        assert_eq!(size_to_option(-5), None);
        assert_eq!(size_to_option(0), Some(0));
        assert_eq!(size_to_option(42), Some(42));
    }

    #[test]
    fn failed_container_cases() {
        assert!(is_failed("restarting", None, "", false));
        assert!(is_failed("dead", Some(0), "", false));
        assert!(is_failed("exited", Some(0), "cannot start", false));
        assert!(is_failed("exited", Some(0), "", true));
        assert!(is_failed("exited", Some(1), "", false));
        for code in [0, 137, 143] {
            assert!(!is_failed("exited", Some(code), "", false), "{code}");
        }
        assert!(!is_failed("exited", None, "", false));
        for s in ["created", "running", "paused"] {
            assert!(!is_failed(s, Some(1), "", false), "{s}");
        }
    }

    #[test]
    fn inspect_only_for_stopped_states() {
        for s in ["exited", "dead", "restarting", "created"] {
            assert!(needs_inspect(s), "{s}");
        }
        for s in ["running", "paused"] {
            assert!(!needs_inspect(s), "{s}");
        }
    }

    #[test]
    fn created_container_is_failed_only_with_a_start_error() {
        assert!(is_failed("created", Some(127), "exec not found", false));
        assert!(!is_failed("created", Some(0), "", false));
    }

    /// 1 回だけ接続を受け、`body` を返す偽の Docker。`None` なら応答せず閉じる。
    fn fake_inspect(path: &Path, body: Option<&'static str>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::UnixListener::bind(path).unwrap();
        tokio::spawn(async move {
            while let Ok((mut conn, _)) = listener.accept().await {
                let mut buf = [0u8; 4096];
                let _ = conn.read(&mut buf).await;
                if let Some(b) = body {
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
                        b.len()
                    );
                    let _ = conn.write_all(resp.as_bytes()).await;
                }
            }
        });
    }

    async fn inspected(body: Option<&'static str>, state: &str) -> bool {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("d.sock");
        fake_inspect(&sock, body);
        Engine::connect(&sock)
            .unwrap()
            .failed_by_inspect("c1", state)
            .await
    }

    #[tokio::test]
    async fn inspect_reports_start_error_of_created_container() {
        let body = r#"{"State":{"Status":"created","Error":"exec: not found","ExitCode":127}}"#;
        assert!(inspected(Some(body), "created").await);
    }

    #[tokio::test]
    async fn inspect_clean_exit_is_not_failed() {
        let body = r#"{"State":{"Status":"exited","Error":"","ExitCode":0}}"#;
        assert!(!inspected(Some(body), "exited").await);
    }

    #[tokio::test]
    async fn inspect_oom_kill_is_failed() {
        let body = r#"{"State":{"Status":"exited","ExitCode":0,"OOMKilled":true}}"#;
        assert!(inspected(Some(body), "exited").await);
    }

    #[tokio::test]
    async fn inspect_failure_falls_back_to_the_listed_state() {
        assert!(inspected(None, "restarting").await);
        assert!(!inspected(None, "exited").await);
    }

    #[tokio::test]
    async fn events_subscribe_only_to_list_changing_actions() {
        use tokio::io::AsyncReadExt;
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("d.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (mut conn, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 4096];
            let n = conn.read(&mut buf).await.unwrap();
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            std::future::pending::<()>().await;
        });
        let engine = Engine::connect(&sock).unwrap();
        let mut stream = std::pin::pin!(engine.events());
        let _ = tokio::time::timeout(Duration::from_millis(200), stream.next()).await;
        let req = rx.await.unwrap();
        for a in EVENT_ACTIONS {
            assert!(req.contains(a), "{a}: {req}");
        }
        for noisy in ["exec_create", "exec_start", "exec_die", "health_status"] {
            assert!(!req.contains(noisy), "{noisy}: {req}");
        }
    }

    #[tokio::test]
    async fn summary_maps_every_list_and_marks_prunable_volumes() {
        use crate::docker::actions::fake::{fake_docker, route};
        let f = fake_docker(vec![
            route(
                "/containers/json",
                r#"[{"Id":"c1","Names":["/web"],"Image":"docker.io/library/nginx:alpine","ImageID":"sha256:aa","State":"running","Status":"Up"}]"#,
            ),
            route(
                "/images/json",
                r#"[{"Id":"sha256:aa","ParentId":"","RepoTags":["nginx:alpine"],"RepoDigests":[],"Created":1000,"Size":2048,"SharedSize":0,"Labels":{},"Containers":0}]"#,
            ),
            route(
                "/volumes",
                r#"{"Volumes":[
                    {"Name":"anon1","Driver":"local","Mountpoint":"/m1","Labels":{"com.docker.volume.anonymous":""},"Options":{},"Scope":"local"},
                    {"Name":"named","Driver":"local","Mountpoint":"/m2","Labels":{},"Options":{},"Scope":"local"},
                    {"Name":"busy","Driver":"local","Mountpoint":"/m3","Labels":{"com.docker.volume.anonymous":""},"Options":{},"Scope":"local"}
                ],"Warnings":[]}"#,
            ),
            route(
                "/networks",
                r#"[{"Id":"n1","Name":"bridge","Driver":"bridge"},{"Id":"n2","Name":"my net","Driver":"bridge"}]"#,
            ),
            route(
                "/system/df",
                r#"{"VolumeUsage":{"TotalSize":30,"Items":[
                    {"Name":"anon1","UsageData":{"Size":10,"RefCount":0}},
                    {"Name":"named","UsageData":{"Size":20,"RefCount":0}},
                    {"Name":"busy","UsageData":{"Size":5,"RefCount":2}}
                ]},"BuildCacheUsage":{"TotalSize":7}}"#,
            ),
        ]);
        let summary = f.engine.summary().await.unwrap();
        let volumes: Vec<_> = summary
            .volumes
            .iter()
            .map(|v| (v.name.as_str(), v.size_bytes, v.prunable))
            .collect();
        assert_eq!(
            volumes,
            [
                ("anon1", Some(10), true),
                ("named", Some(20), false),
                ("busy", Some(5), false)
            ]
        );
        assert_eq!(summary.volumes[0].driver, "local");
        assert_eq!(summary.volumes[0].mountpoint, "/m1");
        assert_eq!(summary.images[0].repo_tags, ["nginx:alpine"]);
        assert_eq!(
            summary.containers[0].image,
            "docker.io/library/nginx:alpine"
        );
        assert_eq!(summary.containers[0].image_id, "sha256:aa");
        assert_eq!(summary.images[0].size_bytes, 2048);
        let nets: Vec<_> = summary
            .networks
            .iter()
            .map(|n| (n.id.as_str(), n.name.as_str(), n.builtin))
            .collect();
        assert_eq!(nets, [("n1", "bridge", true), ("n2", "my net", false)]);
        assert_eq!(summary.disk.build_cache, Some(7));
    }

    #[tokio::test]
    async fn summary_still_lists_volumes_when_df_fails() {
        use crate::docker::actions::fake::{fake_docker, route};
        let f = fake_docker(vec![
            route("/containers/json", "[]"),
            route("/images/json", "[]"),
            route(
                "/volumes",
                r#"{"Volumes":[{"Name":"anon1","Driver":"local","Mountpoint":"/m1","Labels":{"com.docker.volume.anonymous":""},"Options":{},"Scope":"local"}]}"#,
            ),
            route("/networks", "[]"),
        ]);
        let summary = f.engine.summary().await.unwrap();
        // dfが取れないときは、使用中かどうかを決めつけず、片付けの対象にしない。
        assert_eq!(summary.volumes[0].size_bytes, None);
        assert!(!summary.volumes[0].prunable);
    }

    #[test]
    fn only_unused_anonymous_volumes_are_prunable() {
        let volume = |anonymous: bool| Volume {
            name: "v1".into(),
            driver: "local".into(),
            mountpoint: "/m".into(),
            labels: if anonymous {
                HashMap::from([(ANONYMOUS_VOLUME_LABEL.to_string(), String::new())])
            } else {
                HashMap::new()
            },
            ..Default::default()
        };
        let usage = |unused| df::VolumeUse {
            size: Some(9),
            unused,
        };
        let row = volume_row(volume(true), usage(true));
        assert!(row.prunable);
        assert_eq!(
            (
                row.name.as_str(),
                row.driver.as_str(),
                row.mountpoint.as_str()
            ),
            ("v1", "local", "/m")
        );
        assert_eq!(row.size_bytes, Some(9));
        assert!(!volume_row(volume(true), usage(false)).prunable);
        assert!(!volume_row(volume(false), usage(true)).prunable);
    }

    #[test]
    fn container_name_drops_leading_slash() {
        assert_eq!(container_name(&["/web".into(), "/x/alias".into()]), "web");
        assert_eq!(container_name(&[]), "");
    }

    #[test]
    fn error_messages_are_localized_and_carry_data() {
        let e = DockerError::Api("boom".into());
        assert!(e.message(Lang::En).ends_with(": boom"));
        assert!(e.message(Lang::Ja).contains("docker"));
        assert_ne!(e.message(Lang::En), e.message(Lang::Ja));
        for e in [
            DockerError::Connect("x".into()),
            DockerError::InvalidPort("70000:80".into()),
        ] {
            assert!(e.message(Lang::En).contains(": "));
            assert_ne!(e.message(Lang::En), e.message(Lang::Ja));
            assert_eq!(e.to_string(), e.message(Lang::En));
        }
    }

    #[test]
    fn name_accepts_the_names_docker_accepts() {
        for ok in ["ab", "web-1", "my_app.v2", "0a", "A9", &"a".repeat(64)] {
            assert_eq!(check_name(ok), Ok(()), "{ok}");
        }
    }

    #[test]
    fn name_rejects_empty_leading_symbols_and_url_syntax() {
        for bad in [
            "", "a", "-ab", ".ab", "_ab", "a/b", "/ab", "a?b", "a#b", "..", "a b", "a\nb", "aé",
            "a%2f",
        ] {
            assert_eq!(
                check_name(bad),
                Err(DockerError::InvalidId(bad.into())),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn image_ref_accepts_names_tags_digests_and_ids() {
        let digest = format!("repo@sha256:{}", "a1".repeat(32));
        for ok in [
            "nginx",
            "nginx:alpine",
            "ghcr.io/owner/app:1.2",
            "localhost:5000/x",
            "library/ubuntu@sha256:0123abcd0123abcd0123abcd0123abcd0123abcd0123abcd0123abcd0123abcd",
            "sha256:0123abcd0123abcd0123abcd0123abcd0123abcd0123abcd0123abcd0123abcd",
            "0123abcd",
            "a",
            "my_repo/a.b-c:v1_2+x",
            digest.as_str(),
        ] {
            assert_eq!(check_image_ref(ok), Ok(()), "{ok}");
        }
    }

    #[test]
    fn image_ref_length_is_capped_at_the_longest_valid_reference() {
        // 文法上いちばん長い形（名前、タグ、sha512のdigest）が通り、1字でも長ければ拒む。
        let longest = format!(
            "{}:{}@sha512:{}",
            "a".repeat(NAME_TOTAL_LENGTH_MAX),
            "t".repeat(128),
            "0".repeat(128)
        );
        assert_eq!(longest.len(), 520);
        assert_eq!(check_image_ref(&longest), Ok(()));
        let long = format!("{longest}0");
        assert_eq!(
            check_image_ref(&long),
            Err(DockerError::InvalidId(long.clone()))
        );
    }

    #[test]
    fn image_ref_rejects_url_syntax_and_odd_shapes() {
        for bad in [
            "",
            "/nginx",
            "-nginx",
            "a?b",
            "a#b",
            "a b",
            "a\tb",
            "a\nb",
            "a\u{7f}b",
            "a%2fb",
            "x/../y",
            "..",
            "../x",
            "x/..",
            ".",
            "x/./y",
            "a//b",
            "a/",
            "aé",
            "a\\b",
            "x/../../containers/abc/kill?signal=KILL#",
        ] {
            assert_eq!(
                check_image_ref(bad),
                Err(DockerError::InvalidId(bad.into())),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn repo_names_follow_the_distribution_reference_grammar() {
        let long_ok = "a".repeat(255);
        for ok in [
            "app",
            "o/app",
            "ghcr.io/o/app",
            "localhost:5000/x",
            "Registry.Example.com:443/team/app",
            "[::1]:5000/app",
            "[::1]/app",
            "My-Registry.io:5000/app",
            "a.b_c__d---e/f",
            "x-y",
            long_ok.as_str(),
        ] {
            assert_eq!(check_repo(ok), Ok(()), "{ok}");
        }
        let long_bad = "a".repeat(256);
        for bad in [
            "",
            "App",
            "o/App",
            "app:v1",
            "app@sha256:00",
            "-app",
            "app-",
            "a..b",
            "a___b",
            "a._b",
            "o//app",
            "/app",
            "app/",
            "a b",
            "-x.io/app",
            "x.io:port/app",
            "[]/app",
            "[zz]/app",
            "aB",
            "app.",
            long_bad.as_str(),
        ] {
            assert_eq!(
                check_repo(bad),
                Err(DockerError::InvalidId(bad.into())),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn tags_follow_the_distribution_reference_grammar() {
        let max = format!("v{}", "1".repeat(127));
        for ok in ["latest", "v1.2-rc_3", "_x", "V1", "1", max.as_str()] {
            assert_eq!(check_tag(ok), Ok(()), "{ok}");
        }
        let too_long = format!("v{}", "1".repeat(128));
        for bad in ["", ".x", "-x", "a b", "a/b", "a:b", "é", too_long.as_str()] {
            assert_eq!(
                check_tag(bad),
                Err(DockerError::InvalidId(bad.into())),
                "{bad:?}"
            );
        }
    }

    #[tokio::test]
    async fn tag_rejects_a_bad_repo_or_tag_before_calling_docker() {
        use crate::docker::actions::fake::fake_docker;
        let f = fake_docker(vec![]);
        assert!(f.engine.tag("x:1", "App", "v1").await.is_err());
        assert!(f.engine.tag("x:1", "app", "a b").await.is_err());
        assert!(f.seen().is_empty());
    }

    #[test]
    fn volume_name_too_long_message_is_in_both_languages() {
        let e = DockerError::VolumeNameTooLong;
        assert_eq!(
            e.message(Lang::En),
            "Volume name is too long (255 characters at most)"
        );
        assert_eq!(
            e.message(Lang::Ja),
            "ボリュームの名前が長すぎます（255 字まで）"
        );
    }

    #[test]
    fn invalid_id_message_is_in_both_languages() {
        let e = DockerError::InvalidId("a/b".into());
        assert!(
            e.message(Lang::En)
                .contains("invalid name, ID or image reference")
        );
        assert!(e.message(Lang::Ja).contains("名前、ID、イメージ参照"));
        assert!(e.message(Lang::En).contains("\"a/b\""));
    }
}
