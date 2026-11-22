//! コンテナ・イメージ・ボリューム・ネットワークの操作。

mod images;
mod resources;

pub use images::PullProgress;
pub use resources::NetDriver;

use super::{DockerError, Engine, check_container_ref, size_to_option};
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
        check_container_ref(id)?;
        Ok(self
            .docker
            .start_container(id, None::<StartContainerOptions>)
            .await?)
    }

    pub async fn stop(&self, id: &str) -> Result<(), DockerError> {
        check_container_ref(id)?;
        Ok(self
            .docker
            .stop_container(id, None::<StopContainerOptions>)
            .await?)
    }

    pub async fn restart(&self, id: &str) -> Result<(), DockerError> {
        check_container_ref(id)?;
        Ok(self
            .docker
            .restart_container(id, None::<RestartContainerOptions>)
            .await?)
    }

    pub async fn pause(&self, id: &str) -> Result<(), DockerError> {
        check_container_ref(id)?;
        Ok(self.docker.pause_container(id).await?)
    }

    pub async fn unpause(&self, id: &str) -> Result<(), DockerError> {
        check_container_ref(id)?;
        Ok(self.docker.unpause_container(id).await?)
    }

    pub async fn rename(&self, id: &str, new: &str) -> Result<(), DockerError> {
        check_container_ref(id)?;
        check_container_ref(new)?;
        let opts = RenameContainerOptionsBuilder::new().name(new).build();
        Ok(self.docker.rename_container(id, opts).await?)
    }

    pub async fn remove(&self, id: &str, force: bool) -> Result<(), DockerError> {
        check_container_ref(id)?;
        let opts = RemoveContainerOptionsBuilder::new().force(force).build();
        Ok(self.docker.remove_container(id, Some(opts)).await?)
    }

    pub async fn inspect(&self, id: &str) -> Result<serde_json::Value, DockerError> {
        check_container_ref(id)?;
        let r = self
            .docker
            .inspect_container(id, None::<InspectContainerOptions>)
            .await?;
        to_json(&r)
    }

    /// 1 回分の CPU とメモリ。stream=false の応答には、直前の標本（precpu）が入っている。
    pub async fn stats_once(&self, id: &str) -> Result<Usage, DockerError> {
        check_container_ref(id)?;
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
        if let Err(e) = check_container_ref(id) {
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
    async fn a_valid_id_reaches_docker() {
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let (e, count) = counting_engine(dir.path());
        assert!(matches!(e.start("web-1").await, Err(DockerError::Api(_))));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}
