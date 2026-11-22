//! 選んだ colima インスタンスの Docker API への接続と、一覧の取得。

pub mod actions;
mod df;
mod logs;

use crate::i18n::{Lang, Msg};
use crate::snapshot::{ContainerRow, DockerSummary, ImageRow, NetworkRow, VolumeRow};
use bollard::models::{ContainerSummary, PortSummary};
use bollard::query_parameters::{
    EventsOptionsBuilder, ListContainersOptionsBuilder, ListImagesOptions, ListNetworksOptions,
    ListVolumesOptions,
};
use bollard::{API_DEFAULT_VERSION, Docker};
use futures_util::{Stream, StreamExt};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

const REQUEST_TIMEOUT_SECS: u64 = 120;

/// Docker 操作の失敗。文言は `message` で言語を選んで作る。
#[derive(Debug, Clone, PartialEq)]
pub enum DockerError {
    Connect(String),
    /// bollard が返した本文。表示用の文言の後ろにそのまま付ける。
    Api(String),
    InvalidPort(String),
}

impl DockerError {
    pub fn message(&self, lang: Lang) -> String {
        let t = |m: Msg| m.text(lang);
        match self {
            Self::Connect(e) => format!("{}: {e}", t(Msg::DockerConnect)),
            Self::Api(e) => format!("{}: {e}", t(Msg::DockerApi)),
            Self::InvalidPort(s) => format!("{}: {s}", t(Msg::DockerPort)),
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
        let (disk, volume_sizes) = df.unwrap_or_default();
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
                .map(|v| VolumeRow {
                    size_bytes: volume_sizes.get(&v.name).copied().flatten(),
                    name: v.name,
                    driver: v.driver,
                    mountpoint: v.mountpoint,
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
        let filters = HashMap::from([("type", vec!["container", "image", "volume", "network"])]);
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
}
