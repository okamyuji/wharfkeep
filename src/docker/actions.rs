//! コンテナ・イメージ・ボリューム・ネットワークの操作。

#[cfg(test)]
pub(super) mod fake;
mod images;
mod resources;

pub use images::PullProgress;
pub use resources::NetDriver;

use super::{DockerError, Engine, check_name, size_to_option};
use bollard::models::{
    ContainerCpuStats, ContainerCreateBody, HostConfig, PortBinding,
    RestartPolicy as BollardRestartPolicy, RestartPolicyNameEnum,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, InspectContainerOptions, LogsOptionsBuilder,
    RemoveContainerOptionsBuilder, RenameContainerOptionsBuilder, RestartContainerOptions,
    StartContainerOptions, StatsOptionsBuilder, StopContainerOptions,
};
use futures_util::future::Either;
use futures_util::{Stream, StreamExt};
use std::collections::HashMap;

/// 2 回の CPU 標本。`cpu_percent` の入力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuSample {
    pub total: u64,
    pub system: u64,
    pub online_cpus: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proto {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortMap {
    pub host: u16,
    pub container: u16,
    pub proto: Proto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestartPolicy {
    #[default]
    No,
    Always,
    UnlessStopped,
    OnFailure,
}

impl RestartPolicy {
    pub const ALL: [RestartPolicy; 4] = [
        RestartPolicy::No,
        RestartPolicy::Always,
        RestartPolicy::UnlessStopped,
        RestartPolicy::OnFailure,
    ];

    /// `docker run --restart`と同じ名前。
    pub fn as_str(self) -> &'static str {
        match self {
            RestartPolicy::No => "no",
            RestartPolicy::Always => "always",
            RestartPolicy::UnlessStopped => "unless-stopped",
            RestartPolicy::OnFailure => "on-failure",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSpec {
    pub image: String,
    pub name: Option<String>,
    pub ports: Vec<PortMap>,
    pub env: Vec<(String, String)>,
    pub mounts: Vec<Mount>,
    pub restart: RestartPolicy,
    pub cmd: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Usage {
    pub cpu_percent: f64,
    pub mem_bytes: u64,
    pub mem_limit: u64,
}

pub fn cpu_percent(prev: CpuSample, cur: CpuSample) -> f64 {
    let cpu = cur.total.saturating_sub(prev.total);
    let system = cur.system.saturating_sub(prev.system);
    if cpu == 0 || system == 0 {
        return 0.0;
    }
    cpu as f64 / system as f64 * f64::from(cur.online_cpus) * 100.0
}

/// `HOST:CONTAINER` か `HOST:CONTAINER/udp`。`tcp` は省略できる。
pub fn parse_port_map(s: &str) -> Result<PortMap, DockerError> {
    let (ports, proto) = match s.split_once('/') {
        None => (s, Proto::Tcp),
        Some((p, "tcp")) => (p, Proto::Tcp),
        Some((p, "udp")) => (p, Proto::Udp),
        Some(_) => return Err(DockerError::InvalidPort(s.to_string())),
    };
    let parsed = ports
        .split_once(':')
        .and_then(|(h, c)| Some((port_number(h)?, port_number(c)?)));
    match parsed {
        Some((host, container)) => Ok(PortMap {
            host,
            container,
            proto,
        }),
        None => Err(DockerError::InvalidPort(s.to_string())),
    }
}

/// local ドライバはボリューム名をそのままディレクトリ名にするので、Linux の NAME_MAX に合わせる。
const VOLUME_NAME_MAX: usize = 255;

/// 画面で見えないか、前後の文字の向きを変える書式文字（Unicode の Cf のうち入力に紛れやすいもの）。
/// 入力欄で見た値と、実際に渡る値が違って見えるのを防ぐ。
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00ad}'
            | '\u{061c}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

/// `KEY=VALUE`。値は最初の `=` より後ろすべてで、空でもよい。キーは空白と制御文字を含まない。
/// 値は改行を含んでよいが、NULはexecveの環境変数に入らないので拒む。
pub fn parse_env(s: &str) -> Result<(String, String), DockerError> {
    match s.split_once('=') {
        Some((k, v))
            if !k.is_empty()
                && !k.contains(|c: char| c.is_whitespace() || c.is_control())
                && !v.contains('\0')
                && !s.contains(is_invisible) =>
        {
            Ok((k.to_string(), v.to_string()))
        }
        _ => Err(DockerError::InvalidEnv(s.to_string())),
    }
}

/// `SOURCE:TARGET` か `SOURCE:TARGET:ro`（`:rw` も可）。`docker run -v` と同じ形。TARGET は絶対パス。
/// SOURCE は絶対パスか、ボリューム名（moby の RestrictedNamePattern）。`../x` や `~/x` は docker が
/// ボリューム名として拒むので、ここで先に分かりやすく止める。
pub fn parse_mount(s: &str) -> Result<Mount, DockerError> {
    if s.contains(|c: char| c.is_control() || is_invisible(c)) {
        return Err(DockerError::InvalidMount(s.to_string()));
    }
    let parts: Vec<&str> = s.split(':').collect();
    let read_only = match parts.get(2).copied() {
        None | Some("rw") => false,
        Some("ro") => true,
        Some(_) => return Err(DockerError::InvalidMount(s.to_string())),
    };
    match parts[..] {
        [source, target, ..]
            if parts.len() <= 3
                && (source.starts_with('/')
                    || (source.len() <= VOLUME_NAME_MAX && check_name(source).is_ok()))
                && target.starts_with('/') =>
        {
            Ok(Mount {
                source: source.to_string(),
                target: target.to_string(),
                read_only,
            })
        }
        _ => Err(DockerError::InvalidMount(s.to_string())),
    }
}

/// `u16::from_str` は先頭の `+` を許すので、数字だけの文字列に絞る。
fn port_number(s: &str) -> Option<u16> {
    if !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok().filter(|p| *p != 0)
}

impl Proto {
    fn as_str(self) -> &'static str {
        match self {
            Proto::Tcp => "tcp",
            Proto::Udp => "udp",
        }
    }
}

fn create_body(spec: &RunSpec) -> ContainerCreateBody {
    let key = |p: &PortMap| format!("{}/{}", p.container, p.proto.as_str());
    let bindings: HashMap<String, Option<Vec<PortBinding>>> = spec
        .ports
        .iter()
        .map(|p| {
            let b = PortBinding {
                host_ip: None,
                host_port: Some(p.host.to_string()),
            };
            (key(p), Some(vec![b]))
        })
        .collect();
    let binds: Vec<String> = spec
        .mounts
        .iter()
        .map(|m| {
            let ro = if m.read_only { ":ro" } else { "" };
            format!("{}:{}{ro}", m.source, m.target)
        })
        .collect();
    let policy = match spec.restart {
        RestartPolicy::No => RestartPolicyNameEnum::NO,
        RestartPolicy::Always => RestartPolicyNameEnum::ALWAYS,
        RestartPolicy::UnlessStopped => RestartPolicyNameEnum::UNLESS_STOPPED,
        RestartPolicy::OnFailure => RestartPolicyNameEnum::ON_FAILURE,
    };
    ContainerCreateBody {
        image: Some(spec.image.clone()),
        env: (!spec.env.is_empty())
            .then(|| spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
        cmd: spec.cmd.clone(),
        exposed_ports: (!bindings.is_empty()).then(|| bindings.keys().cloned().collect()),
        host_config: Some(HostConfig {
            binds: (!binds.is_empty()).then_some(binds),
            port_bindings: (!bindings.is_empty()).then_some(bindings),
            restart_policy: Some(BollardRestartPolicy {
                name: Some(policy),
                maximum_retry_count: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// `docker stats` と同じく、回収できるページキャッシュを使用量から引く。
fn mem_used(usage: u64, inactive_file: u64) -> u64 {
    usage.saturating_sub(inactive_file)
}

fn cpu_sample(s: &ContainerCpuStats) -> CpuSample {
    CpuSample {
        total: s
            .cpu_usage
            .as_ref()
            .and_then(|u| u.total_usage)
            .unwrap_or(0),
        system: s.system_cpu_usage.unwrap_or(0),
        online_cpus: s.online_cpus.unwrap_or(1),
    }
}

pub(super) fn to_json<T: serde::Serialize>(v: &T) -> Result<serde_json::Value, DockerError> {
    serde_json::to_value(v).map_err(|e| DockerError::Api(e.to_string()))
}

pub(super) fn reclaimed(bytes: Option<i64>) -> u64 {
    bytes.and_then(size_to_option).unwrap_or(0)
}

pub(super) fn filter(
    key: &'static str,
    value: &'static str,
) -> HashMap<&'static str, Vec<&'static str>> {
    HashMap::from([(key, vec![value])])
}

impl Engine {
    pub async fn start(&self, id: &str) -> Result<(), DockerError> {
        check_name(id)?;
        Ok(self
            .docker
            .start_container(id, None::<StartContainerOptions>)
            .await?)
    }

    pub async fn stop(&self, id: &str) -> Result<(), DockerError> {
        check_name(id)?;
        Ok(self
            .docker
            .stop_container(id, None::<StopContainerOptions>)
            .await?)
    }

    pub async fn restart(&self, id: &str) -> Result<(), DockerError> {
        check_name(id)?;
        Ok(self
            .docker
            .restart_container(id, None::<RestartContainerOptions>)
            .await?)
    }

    pub async fn pause(&self, id: &str) -> Result<(), DockerError> {
        check_name(id)?;
        Ok(self.docker.pause_container(id).await?)
    }

    pub async fn unpause(&self, id: &str) -> Result<(), DockerError> {
        check_name(id)?;
        Ok(self.docker.unpause_container(id).await?)
    }

    pub async fn rename(&self, id: &str, new: &str) -> Result<(), DockerError> {
        check_name(id)?;
        check_name(new)?;
        let opts = RenameContainerOptionsBuilder::new().name(new).build();
        Ok(self.docker.rename_container(id, opts).await?)
    }

    pub async fn remove(&self, id: &str, force: bool) -> Result<(), DockerError> {
        check_name(id)?;
        let opts = RemoveContainerOptionsBuilder::new().force(force).build();
        Ok(self.docker.remove_container(id, Some(opts)).await?)
    }

    pub async fn inspect(&self, id: &str) -> Result<serde_json::Value, DockerError> {
        check_name(id)?;
        let r = self
            .docker
            .inspect_container(id, None::<InspectContainerOptions>)
            .await?;
        to_json(&r)
    }

    /// 1 回分の CPU とメモリ。stream=false の応答には、直前の標本（precpu）が入っている。
    pub async fn stats_once(&self, id: &str) -> Result<Usage, DockerError> {
        check_name(id)?;
        let opts = StatsOptionsBuilder::new()
            .stream(false)
            .one_shot(false)
            .build();
        let r = self
            .docker
            .stats(id, Some(opts))
            .next()
            .await
            .ok_or_else(|| DockerError::Api("empty stats stream".into()))??;
        let cur = r.cpu_stats.unwrap_or_default();
        let prev = r.precpu_stats.unwrap_or_default();
        let mem = r.memory_stats.unwrap_or_default();
        let inactive = mem
            .stats
            .as_ref()
            .and_then(|m| {
                m.get("inactive_file")
                    .or_else(|| m.get("total_inactive_file"))
            })
            .copied()
            .unwrap_or(0);
        Ok(Usage {
            cpu_percent: cpu_percent(cpu_sample(&prev), cpu_sample(&cur)),
            mem_bytes: mem_used(mem.usage.unwrap_or(0), inactive),
            mem_limit: mem.limit.unwrap_or(0),
        })
    }

    /// 1 行ずつ流す。行数の上限は呼び出し側が決める。
    pub fn logs(
        &self,
        id: &str,
        follow: bool,
    ) -> impl Stream<Item = Result<String, DockerError>> + '_ {
        if let Err(e) = check_name(id) {
            return Either::Left(futures_util::stream::once(std::future::ready(Err(e))));
        }
        let opts = LogsOptionsBuilder::new()
            .stdout(true)
            .stderr(true)
            .follow(follow)
            .build();
        Either::Right(super::logs::split_log_stream(Box::pin(
            self.docker.logs(id, Some(opts)),
        )))
    }

    /// 作って開始する。開始に失敗したら作ったコンテナを消し、開始の失敗を返す。
    pub async fn run(&self, spec: RunSpec) -> Result<String, DockerError> {
        let opts = spec
            .name
            .as_deref()
            .map(|n| CreateContainerOptionsBuilder::new().name(n).build());
        let id = self
            .docker
            .create_container(opts, create_body(&spec))
            .await?
            .id;
        if let Err(e) = self.start(&id).await {
            // 消せなくても、返すのは開始の失敗。
            let _ = self.remove(&id, true).await;
            return Err(e);
        }
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(total: u64, system: u64, cpus: u32) -> CpuSample {
        CpuSample {
            total,
            system,
            online_cpus: cpus,
        }
    }

    #[test]
    fn cpu_percent_from_two_samples() {
        // 容器が使った 50 / システム全体の 200、4 コア -> 50% * 4 = 100%。
        let p = cpu_percent(sample(100, 1000, 4), sample(150, 1200, 4));
        assert!((p - 100.0).abs() < 1e-9, "{p}");
        let p = cpu_percent(sample(0, 0, 2), sample(10, 400, 2));
        assert!((p - 5.0).abs() < 1e-9, "{p}");
    }

    #[test]
    fn cpu_percent_is_zero_without_a_delta() {
        assert_eq!(cpu_percent(sample(5, 100, 4), sample(9, 100, 4)), 0.0);
        assert_eq!(cpu_percent(sample(5, 100, 4), sample(5, 200, 4)), 0.0);
        // カウンタが巻き戻っても負にしない。
        assert_eq!(cpu_percent(sample(9, 100, 4), sample(5, 200, 4)), 0.0);
        assert_eq!(cpu_percent(sample(9, 300, 4), sample(5, 200, 4)), 0.0);
    }

    #[test]
    fn parse_port_map_valid_and_invalid() {
        let m = parse_port_map("8080:80").unwrap();
        assert_eq!((m.host, m.container, m.proto), (8080, 80, Proto::Tcp));
        let m = parse_port_map("53:53/udp").unwrap();
        assert_eq!((m.host, m.container, m.proto), (53, 53, Proto::Udp));
        assert_eq!(parse_port_map("1:65535/tcp").unwrap().container, 65535);
        for bad in [
            "80",
            "a:b",
            "70000:80",
            "0:80",
            "80:0",
            "80:80/sctp",
            "80:80/",
            "8080:80/UDP:x",
            "1:2:3",
            "",
            ":",
            "-1:80",
            " 80:80",
        ] {
            assert!(
                matches!(parse_port_map(bad), Err(DockerError::InvalidPort(ref s)) if s == bad),
                "{bad:?}"
            );
        }
    }

    fn spec() -> RunSpec {
        RunSpec {
            image: "nginx:alpine".into(),
            name: Some("web".into()),
            ports: vec![
                parse_port_map("8080:80").unwrap(),
                parse_port_map("53:53/udp").unwrap(),
            ],
            env: vec![("A".into(), "b=c".into())],
            mounts: vec![
                Mount {
                    source: "data".into(),
                    target: "/d".into(),
                    read_only: false,
                },
                Mount {
                    source: "/etc/x".into(),
                    target: "/x".into(),
                    read_only: true,
                },
            ],
            restart: RestartPolicy::UnlessStopped,
            cmd: Some(vec!["sleep".into(), "1".into()]),
        }
    }

    #[test]
    fn parse_env_splits_at_the_first_equals_sign() {
        assert_eq!(parse_env("A=1"), Ok(("A".into(), "1".into())));
        assert_eq!(parse_env("URL=a=b c"), Ok(("URL".into(), "a=b c".into())));
        assert_eq!(parse_env("EMPTY="), Ok(("EMPTY".into(), String::new())));
        assert_eq!(parse_env("MULTI=a\nb"), Ok(("MULTI".into(), "a\nb".into())));
        for bad in [
            "",
            "=x",
            "noequals",
            "A B=1",
            "\tA=1",
            "A\0B=1",
            "A\u{1b}=1",
            "A=x\0y",
            "A\u{202e}=1",
            "A=x\u{202e}y",
            "A\u{200b}=1",
            "A=\u{feff}",
            "A\u{00ad}=1",
            "A\u{061c}=1",
            "A\u{180e}=1",
            "A\u{200f}=1",
            "A\u{202a}=1",
            "A\u{2060}=1",
            "A\u{2064}=1",
            "A\u{2069}=1",
            "A\u{206a}=1",
            "A=\u{206f}",
            "A\u{e0001}=1",
            "A=\u{e0020}",
            "A=\u{e007f}",
            "A\u{fff9}=1",
            "A=\u{fffb}",
        ] {
            assert_eq!(
                parse_env(bad),
                Err(DockerError::InvalidEnv(bad.into())),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn parse_mount_takes_source_target_and_an_optional_mode() {
        let m = |source: &str, target: &str, read_only| Mount {
            source: source.into(),
            target: target.into(),
            read_only,
        };
        assert_eq!(parse_mount("/host:/data"), Ok(m("/host", "/data", false)));
        assert_eq!(parse_mount("vol:/v:ro"), Ok(m("vol", "/v", true)));
        let longest = "v".repeat(VOLUME_NAME_MAX);
        assert_eq!(
            parse_mount(&format!("{longest}:/d")),
            Ok(m(&longest, "/d", false))
        );
        assert_eq!(parse_mount("vol:/v:rw"), Ok(m("vol", "/v", false)));
        for bad in [
            "",
            "x",
            ":/b",
            "src:relative",
            "a:/b:xx",
            "a:/b:ro:x",
            "a:",
            "v:/data",
            "../x:/y",
            "~/d:/y",
            "./d:/y",
            "-v:/y",
            "/a\0b:/y",
            "/a:/b\0",
            "/a:/b\n",
            "/a\u{202e}b:/y",
            "/a:/b\u{2066}",
            "/a\u{e0041}:/b",
            "/a:/b\u{fffa}",
            &format!("{}:/d", "v".repeat(VOLUME_NAME_MAX + 1)),
        ] {
            assert_eq!(
                parse_mount(bad),
                Err(DockerError::InvalidMount(bad.into())),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn env_and_mount_errors_are_in_both_languages() {
        use crate::i18n::Lang;
        let env = DockerError::InvalidEnv("x".into());
        assert!(env.message(Lang::En).contains("KEY=VALUE"));
        assert!(env.message(Lang::Ja).contains("環境変数"));
        let mount = DockerError::InvalidMount("x".into());
        assert!(mount.message(Lang::En).contains("mount"));
        assert!(mount.message(Lang::Ja).contains("マウント"));
    }

    #[test]
    fn create_body_maps_every_field() {
        let b = create_body(&spec());
        assert_eq!(b.image.as_deref(), Some("nginx:alpine"));
        assert_eq!(b.env, Some(vec!["A=b=c".to_string()]));
        assert_eq!(b.cmd, Some(vec!["sleep".to_string(), "1".to_string()]));
        let mut exposed = b.exposed_ports.clone().unwrap();
        exposed.sort();
        assert_eq!(exposed, ["53/udp", "80/tcp"]);
        let h = b.host_config.unwrap();
        assert_eq!(
            h.binds,
            Some(vec!["data:/d".to_string(), "/etc/x:/x:ro".to_string()])
        );
        let pb = h.port_bindings.unwrap();
        let tcp = pb["80/tcp"].as_ref().unwrap();
        assert_eq!(tcp[0].host_port.as_deref(), Some("8080"));
        assert_eq!(
            pb["53/udp"].as_ref().unwrap()[0].host_port.as_deref(),
            Some("53")
        );
        let rp = h.restart_policy.unwrap();
        assert_eq!(
            rp.name,
            Some(bollard::models::RestartPolicyNameEnum::UNLESS_STOPPED)
        );
    }

    #[test]
    fn create_body_for_minimal_spec_has_no_optional_parts() {
        let b = create_body(&RunSpec {
            image: "x".into(),
            name: None,
            ports: vec![],
            env: vec![],
            mounts: vec![],
            restart: RestartPolicy::No,
            cmd: None,
        });
        assert_eq!(b.env, None);
        assert_eq!(b.cmd, None);
        let h = b.host_config.unwrap();
        assert_eq!(h.binds, None);
        assert_eq!(
            h.restart_policy.unwrap().name,
            Some(bollard::models::RestartPolicyNameEnum::NO)
        );
    }

    #[test]
    fn restart_policy_names() {
        use bollard::models::RestartPolicyNameEnum as E;
        for (p, e) in [
            (RestartPolicy::No, E::NO),
            (RestartPolicy::Always, E::ALWAYS),
            (RestartPolicy::UnlessStopped, E::UNLESS_STOPPED),
            (RestartPolicy::OnFailure, E::ON_FAILURE),
        ] {
            let mut s = spec();
            s.restart = p;
            let rp = create_body(&s).host_config.unwrap().restart_policy.unwrap();
            assert_eq!(rp.name, Some(e));
        }
    }

    #[test]
    fn to_json_serializes_the_value() {
        let v = to_json(&vec![1, 2]).unwrap();
        assert_eq!(v, serde_json::json!([1, 2]));
    }

    #[test]
    fn reclaimed_bytes_ignore_missing_and_negative() {
        assert_eq!(reclaimed(Some(42)), 42);
        assert_eq!(reclaimed(Some(-1)), 0);
        assert_eq!(reclaimed(None), 0);
    }

    #[test]
    fn filter_builds_a_single_entry() {
        let f = filter("dangling", "true");
        assert_eq!(f.len(), 1);
        assert_eq!(f["dangling"], ["true"]);
    }

    #[test]
    fn mem_used_excludes_inactive_file_cache() {
        assert_eq!(mem_used(100, 30), 70);
        assert_eq!(mem_used(10, 30), 0);
    }

    #[test]
    fn cpu_sample_reads_stats() {
        let s = ContainerCpuStats {
            cpu_usage: Some(bollard::models::ContainerCpuUsage {
                total_usage: Some(7),
                ..Default::default()
            }),
            system_cpu_usage: Some(9),
            online_cpus: Some(3),
            ..Default::default()
        };
        assert_eq!(cpu_sample(&s), sample(7, 9, 3));
        assert_eq!(cpu_sample(&ContainerCpuStats::default()), sample(0, 0, 1));
    }

    /// 接続を数えるだけの偽 UNIX ソケット。不正な ID が bollard に渡らなければ 0 のまま。
    fn counting_engine(
        dir: &std::path::Path,
    ) -> (Engine, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let sock = dir.join("docker.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        let count = std::sync::Arc::new(AtomicUsize::new(0));
        let seen = count.clone();
        tokio::spawn(async move {
            while let Ok((conn, _)) = listener.accept().await {
                seen.fetch_add(1, Ordering::SeqCst);
                drop(conn);
            }
        });
        (Engine::connect(&sock).unwrap(), count)
    }

    #[tokio::test]
    async fn every_operation_rejects_a_bad_id_without_calling_docker() {
        use futures_util::StreamExt;
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let (e, count) = counting_engine(dir.path());
        let bad = "../images/json?x=1#";
        let want = DockerError::InvalidId(bad.into());
        assert_eq!(e.start(bad).await, Err(want.clone()));
        assert_eq!(e.stop(bad).await, Err(want.clone()));
        assert_eq!(e.restart(bad).await, Err(want.clone()));
        assert_eq!(e.pause(bad).await, Err(want.clone()));
        assert_eq!(e.unpause(bad).await, Err(want.clone()));
        assert_eq!(e.rename(bad, "ok-name").await, Err(want.clone()));
        assert_eq!(e.remove(bad, true).await, Err(want.clone()));
        assert_eq!(e.inspect(bad).await, Err(want.clone()));
        assert_eq!(e.stats_once(bad).await.map(|_| ()), Err(want.clone()));
        let first = std::pin::pin!(e.logs(bad, false)).next().await.unwrap();
        assert_eq!(first, Err(want.clone()));
        assert!(!e.failed_by_inspect(bad, "exited").await);
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn rename_rejects_a_bad_new_name_without_calling_docker() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let (e, count) = counting_engine(dir.path());
        assert_eq!(
            e.rename("web-1", "a/b").await,
            Err(DockerError::InvalidId("a/b".into()))
        );
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn volume_network_and_image_operations_reject_bad_names_without_calling_docker() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let (e, count) = counting_engine(dir.path());
        let bad = "x/../../containers/abc/kill?signal=KILL#";
        let want = DockerError::InvalidId(bad.into());
        assert_eq!(e.remove_volume(bad).await, Err(want.clone()));
        assert_eq!(e.inspect_volume(bad).await, Err(want.clone()));
        assert_eq!(e.remove_network(bad).await, Err(want.clone()));
        assert_eq!(e.inspect_network(bad).await, Err(want.clone()));
        assert_eq!(e.remove_image(bad, true).await, Err(want.clone()));
        assert_eq!(e.inspect_image(bad).await, Err(want.clone()));
        assert_eq!(e.tag(bad, "app", "1").await, Err(want.clone()));
        assert_eq!(e.tag("nginx", bad, "1").await, Err(want.clone()));
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn valid_volume_network_and_image_names_reach_docker() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let (e, count) = counting_engine(dir.path());
        let digest = format!("repo@sha256:{}", "a1".repeat(32));
        let mut sent = 0;
        for r in [
            "nginx:alpine",
            "ghcr.io/owner/app:1.2",
            "localhost:5000/x",
            &digest,
        ] {
            assert!(
                matches!(e.inspect_image(r).await, Err(DockerError::Api(_))),
                "{r}"
            );
            assert!(matches!(
                e.remove_image(r, false).await,
                Err(DockerError::Api(_))
            ));
            sent += 2;
        }
        assert!(matches!(
            e.tag("nginx:alpine", "ghcr.io/o/app", "1").await,
            Err(DockerError::Api(_))
        ));
        assert!(matches!(
            e.remove_volume("data-1").await,
            Err(DockerError::Api(_))
        ));
        assert!(matches!(
            e.inspect_volume("data-1").await,
            Err(DockerError::Api(_))
        ));
        assert!(matches!(
            e.remove_network("net_1").await,
            Err(DockerError::Api(_))
        ));
        assert!(matches!(
            e.inspect_network("net_1").await,
            Err(DockerError::Api(_))
        ));
        assert_eq!(count.load(Ordering::SeqCst), sent + 5);
    }

    #[tokio::test]
    async fn a_valid_id_reaches_docker() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let (e, count) = counting_engine(dir.path());
        assert!(matches!(e.start("web-1").await, Err(DockerError::Api(_))));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}
