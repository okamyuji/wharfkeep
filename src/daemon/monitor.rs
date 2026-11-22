//! 監視ループ。colimaの一覧を周期的に取り、実行中のインスタンスごとにDockerのイベントを購読して状態ファイルへ書く。

use crate::colima::{Colima, ColimaError, Instance, InstanceStatus};
use crate::docker::Engine;
use crate::i18n::{Lang, Msg};
use crate::snapshot::{self, DockerState, Snapshot};
use futures_util::StreamExt;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// colimaの一覧とDockerのソケットの取り先。試験では偽物を渡す。
pub(crate) trait InstanceSource {
    async fn list(&self) -> Result<Vec<Instance>, ColimaError>;
    async fn docker_socket(&self, profile: &str) -> Result<PathBuf, ColimaError>;
}

impl InstanceSource for Colima {
    async fn list(&self) -> Result<Vec<Instance>, ColimaError> {
        Colima::list(self).await
    }
    async fn docker_socket(&self, profile: &str) -> Result<PathBuf, ColimaError> {
        Colima::docker_socket(self, profile).await
    }
}

/// 1つのインスタンスのDockerへの接続と、そのイベントを読むタスク。dropでタスクを止める。
struct Watch {
    engine: Engine,
    /// イベントが来て、一覧を取り直す必要がある。
    dirty: Arc<AtomicBool>,
    /// 購読が切れた理由。切れていなければ`None`。
    ended: Arc<Mutex<Option<String>>>,
    task: JoinHandle<()>,
    last: DockerState,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Watch {
    fn start(engine: Engine, wake: Arc<Notify>, lang: Lang) -> Watch {
        let dirty = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(Mutex::new(None));
        let task = tokio::spawn(read_events(
            engine.clone(),
            dirty.clone(),
            ended.clone(),
            wake,
            lang,
        ));
        Watch {
            engine,
            dirty,
            ended,
            task,
            last: DockerState::Unavailable {
                reason: String::new(),
            },
        }
    }
}

async fn read_events(
    engine: Engine,
    dirty: Arc<AtomicBool>,
    ended: Arc<Mutex<Option<String>>>,
    wake: Arc<Notify>,
    lang: Lang,
) {
    let events = engine.events();
    let mut events = std::pin::pin!(events);
    let reason = loop {
        match events.next().await {
            Some(Ok(())) => {
                dirty.store(true, Ordering::SeqCst);
                wake.notify_one();
            }
            Some(Err(e)) => break e.message(lang),
            None => break Msg::DockerEventsEnded.text(lang).to_string(),
        }
    };
    *ended.lock().unwrap_or_else(|p| p.into_inner()) = Some(reason);
    wake.notify_one();
}

async fn summarize(engine: &Engine, lang: Lang) -> DockerState {
    match engine.summary().await {
        Ok(s) => DockerState::Ready(s),
        Err(e) => DockerState::Unavailable {
            reason: e.message(lang),
        },
    }
}

/// 実行中のインスタンスごとのDockerへの接続とイベントの購読。
pub(crate) struct Engines {
    watches: HashMap<String, Watch>,
    wake: Arc<Notify>,
    lang: Lang,
}

impl Engines {
    pub(crate) fn new(wake: Arc<Notify>, lang: Lang) -> Engines {
        Engines {
            watches: HashMap::new(),
            wake,
            lang,
        }
    }

    async fn state(&mut self, src: &impl InstanceSource, name: &str) -> DockerState {
        if let Some(w) = self.watches.get_mut(name) {
            let ended = w.ended.lock().unwrap_or_else(|p| p.into_inner()).take();
            if let Some(reason) = ended {
                self.watches.remove(name);
                return DockerState::Unavailable { reason };
            }
            if w.dirty.swap(false, Ordering::SeqCst) {
                w.last = summarize(&w.engine, self.lang).await;
                if matches!(w.last, DockerState::Unavailable { .. }) {
                    // 取れなかったので次の周期に取り直す。
                    w.dirty.store(true, Ordering::SeqCst);
                }
            }
            return w.last.clone();
        }
        self.connect(src, name).await
    }

    async fn connect(&mut self, src: &impl InstanceSource, name: &str) -> DockerState {
        let unavailable = |reason| DockerState::Unavailable { reason };
        let socket = match src.docker_socket(name).await {
            Ok(s) => s,
            Err(e) => return unavailable(e.message(self.lang)),
        };
        let engine = match Engine::connect(&socket) {
            Ok(e) => e,
            Err(e) => return unavailable(e.message(self.lang)),
        };
        // 一覧を取る前に購読を始める。逆にすると、その間の変化を取りこぼす。
        let mut w = Watch::start(engine, self.wake.clone(), self.lang);
        w.last = summarize(&w.engine, self.lang).await;
        let st = w.last.clone();
        if matches!(st, DockerState::Ready(_)) {
            self.watches.insert(name.to_string(), w);
        }
        st
    }
}

/// 監視の1周期。一覧を取れなければ`Err`を返す。
pub(crate) async fn tick(
    src: &impl InstanceSource,
    engines: &mut Engines,
) -> Result<Snapshot, ColimaError> {
    let instances = src.list().await?;
    engines.watches.retain(|name, _| {
        instances
            .iter()
            .any(|i| &i.name == name && i.status == InstanceStatus::Running)
    });
    let mut docker = BTreeMap::new();
    for i in &instances {
        let st = if i.status == InstanceStatus::Running {
            engines.state(src, &i.name).await
        } else {
            DockerState::Unavailable {
                reason: format!(
                    "{}: {}",
                    Msg::InstanceNotRunning.text(engines.lang),
                    String::from(i.status.clone())
                ),
            }
        };
        docker.insert(i.name.clone(), st);
    }
    Ok(Snapshot {
        updated_at: SystemTime::now(),
        instances,
        docker,
    })
}

/// 変化がなくても、この間隔で一覧を取り直して状態ファイルの時刻を進める。
const INTERVAL: Duration = Duration::from_secs(5);
/// イベントは続けて来るので、起こされてから少し待ってまとめて読む。
const DEBOUNCE: Duration = Duration::from_millis(300);

/// 監視を続ける。周期ごとに状態ファイルを書き、`publish`へ渡す。戻らない。
pub(crate) async fn run_loop(
    src: Colima,
    state_path: PathBuf,
    wake: Arc<Notify>,
    lang: Lang,
    publish: impl Fn(Snapshot),
) {
    let mut engines = Engines::new(wake.clone(), lang);
    let mut last: Option<Snapshot> = None;
    loop {
        let snap = match tick(&src, &mut engines).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("wharfkeep: {}", e.message(lang));
                carry_over(last.take())
            }
        };
        if let Err(e) = snapshot::write_atomic(&state_path, &snap) {
            eprintln!(
                "wharfkeep: {}: {}: {e}",
                Msg::SnapshotWriteFailed.text(lang),
                state_path.display()
            );
        }
        publish(snap.clone());
        last = Some(snap);
        tokio::select! {
            () = tokio::time::sleep(INTERVAL) => {}
            () = wake.notified() => tokio::time::sleep(DEBOUNCE).await,
        }
    }
}

/// 一覧を取れなくてもデーモンは動いているので、前回の中身のまま時刻だけ進める。
fn carry_over(last: Option<Snapshot>) -> Snapshot {
    Snapshot {
        updated_at: SystemTime::now(),
        ..last.unwrap_or(Snapshot {
            updated_at: SystemTime::now(),
            instances: Vec::new(),
            docker: BTreeMap::new(),
        })
    }
}

/// メニューから選ばれた`colima start|stop <name>`を走らせる。終わったら監視を起こして結果を映す。
/// 失敗したら理由を標準エラーに出して返す。
pub(crate) async fn run_action(
    colima: Colima,
    args: Vec<String>,
    wake: Arc<Notify>,
    lang: Lang,
) -> Option<String> {
    let (mut rx, handle) = colima.run_streaming(args.clone());
    // 失敗の理由として最後の数行だけ残す。
    let mut tail = VecDeque::new();
    while let Some(line) = rx.recv().await {
        if tail.len() == ACTION_TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(line);
    }
    let detail = match handle.await {
        Ok(Ok(status)) if status.success() => None,
        Ok(Ok(status)) => Some(format!("{status}: {}", Vec::from(tail).join(" / "))),
        Ok(Err(e)) => Some(e.message(lang)),
        Err(e) => Some(e.to_string()),
    };
    if let Some(d) = &detail {
        eprintln!(
            "wharfkeep: {}: colima {}: {d}",
            Msg::TrayActionFailed.text(lang),
            args.join(" ")
        );
    }
    wake.notify_one();
    detail
}

const ACTION_TAIL_LINES: usize = 5;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{UnixListener, UnixStream};
    use tokio::sync::mpsc;

    struct Fake {
        instances: Mutex<Result<Vec<Instance>, ColimaError>>,
        socket: Mutex<Result<PathBuf, ColimaError>>,
    }

    impl Fake {
        fn new(instances: Vec<Instance>, socket: Result<PathBuf, ColimaError>) -> Fake {
            Fake {
                instances: Mutex::new(Ok(instances)),
                socket: Mutex::new(socket),
            }
        }
        fn set_instances(&self, v: Result<Vec<Instance>, ColimaError>) {
            *self.instances.lock().unwrap() = v;
        }
    }

    impl InstanceSource for Fake {
        async fn list(&self) -> Result<Vec<Instance>, ColimaError> {
            self.instances.lock().unwrap().clone()
        }
        async fn docker_socket(&self, _profile: &str) -> Result<PathBuf, ColimaError> {
            self.socket.lock().unwrap().clone()
        }
    }

    fn instance(name: &str, status: InstanceStatus) -> Instance {
        Instance {
            name: name.into(),
            status,
            arch: "aarch64".into(),
            cpus: 2,
            memory_bytes: 1,
            disk_bytes: 1,
            runtime: "docker".into(),
        }
    }

    fn engines() -> Engines {
        Engines::new(Arc::new(Notify::new()), Lang::En)
    }

    fn unavailable_reason(s: &Snapshot, name: &str) -> String {
        match &s.docker[name] {
            DockerState::Unavailable { reason } => reason.clone(),
            other => panic!("{name}: {other:?}"),
        }
    }

    fn container_names(s: &Snapshot, name: &str) -> Vec<String> {
        match &s.docker[name] {
            DockerState::Ready(sum) => sum.containers.iter().map(|c| c.name.clone()).collect(),
            other => panic!("{name}: {other:?}"),
        }
    }

    #[tokio::test]
    async fn stopped_instance_written_as_unavailable() {
        let fake = Fake::new(
            vec![instance("default", InstanceStatus::Stopped)],
            Err(ColimaError::InvalidSocket("unused".into())),
        );
        let s = tick(&fake, &mut engines()).await.unwrap();
        assert_eq!(s.instances, [instance("default", InstanceStatus::Stopped)]);
        let reason = unavailable_reason(&s, "default");
        assert!(reason.contains("the instance is not running"), "{reason}");
        assert!(reason.contains("Stopped"), "{reason}");
    }

    #[tokio::test]
    async fn list_failure_is_returned() {
        let fake = Fake::new(vec![], Err(ColimaError::InvalidSocket("unused".into())));
        fake.set_instances(Err(ColimaError::OutputTooLarge));
        assert_eq!(
            tick(&fake, &mut engines()).await.unwrap_err(),
            ColimaError::OutputTooLarge
        );
    }

    #[tokio::test]
    async fn socket_lookup_failure_is_unavailable_with_reason() {
        let fake = Fake::new(
            vec![instance("default", InstanceStatus::Running)],
            Err(ColimaError::InvalidSocket("tcp://x".into())),
        );
        let s = tick(&fake, &mut engines()).await.unwrap();
        let reason = unavailable_reason(&s, "default");
        assert!(
            reason.contains("unexpected docker socket address: tcp://x"),
            "{reason}"
        );
    }

    #[tokio::test]
    async fn missing_socket_is_unavailable_and_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Fake::new(
            vec![instance("default", InstanceStatus::Running)],
            Ok(dir.path().join("no.sock")),
        );
        let mut e = engines();
        let s = tick(&fake, &mut e).await.unwrap();
        let reason = unavailable_reason(&s, "default");
        assert!(
            reason.contains("cannot connect to the docker socket"),
            "{reason}"
        );
        assert!(e.watches.is_empty());
    }

    #[tokio::test]
    async fn docker_api_error_is_unavailable_and_not_kept() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("docker.sock");
        // 接続を受けてすぐ閉じる。
        let listener = UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            while let Ok((conn, _)) = listener.accept().await {
                drop(conn);
            }
        });
        let fake = Fake::new(vec![instance("default", InstanceStatus::Running)], Ok(sock));
        let mut e = engines();
        let s = tick(&fake, &mut e).await.unwrap();
        let reason = unavailable_reason(&s, "default");
        assert!(reason.contains("docker returned an error"), "{reason}");
        assert!(e.watches.is_empty());
    }

    /// Docker APIの偽物。一覧は空で、コンテナの一覧だけ差し替えられる。`/events`は開いたままにし、
    /// 試験から送る行を流す。
    struct FakeDocker {
        containers: Arc<Mutex<String>>,
        events: mpsc::UnboundedReceiver<mpsc::UnboundedSender<Option<String>>>,
    }

    impl FakeDocker {
        fn start(path: &Path) -> FakeDocker {
            let listener = UnixListener::bind(path).unwrap();
            let containers = Arc::new(Mutex::new("[]".to_string()));
            let (ev_tx, ev_rx) = mpsc::unbounded_channel();
            let list = containers.clone();
            tokio::spawn(async move {
                while let Ok((conn, _)) = listener.accept().await {
                    tokio::spawn(serve(conn, list.clone(), ev_tx.clone()));
                }
            });
            FakeDocker {
                containers,
                events: ev_rx,
            }
        }

        /// 次に開かれたイベントの購読の送り口。`None`を送ると接続を閉じる。
        async fn next_subscriber(&mut self) -> mpsc::UnboundedSender<Option<String>> {
            tokio::time::timeout(Duration::from_secs(5), self.events.recv())
                .await
                .expect("events subscribed")
                .unwrap()
        }
    }

    async fn serve(
        mut conn: UnixStream,
        containers: Arc<Mutex<String>>,
        events: mpsc::UnboundedSender<mpsc::UnboundedSender<Option<String>>>,
    ) {
        let Some(line) = request_line(&mut conn).await else {
            return;
        };
        if line.contains("/events") {
            let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n";
            if conn.write_all(head.as_bytes()).await.is_err() {
                return;
            }
            let (tx, mut rx) = mpsc::unbounded_channel();
            let _ = events.send(tx);
            while let Some(Some(ev)) = rx.recv().await {
                let chunk = format!("{:x}\r\n{ev}\r\n", ev.len());
                if conn.write_all(chunk.as_bytes()).await.is_err() {
                    return;
                }
            }
            let _ = conn.write_all(b"0\r\n\r\n").await;
            return;
        }
        let body = if line.contains("/containers/json") {
            containers.lock().unwrap().clone()
        } else {
            fixed_body(&line).to_string()
        };
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let _ = conn.write_all(resp.as_bytes()).await;
    }

    async fn request_line(conn: &mut UnixStream) -> Option<String> {
        let mut req = Vec::new();
        let mut buf = [0u8; 4096];
        while !req.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = conn.read(&mut buf).await.ok().filter(|&n| n > 0)?;
            req.extend_from_slice(&buf[..n]);
        }
        String::from_utf8_lossy(&req)
            .lines()
            .next()
            .map(String::from)
    }

    fn fixed_body(line: &str) -> &'static str {
        if line.contains("/volumes") {
            r#"{"Volumes":[],"Warnings":[]}"#
        } else if line.contains("/system/df") {
            "{}"
        } else {
            "[]"
        }
    }

    const ONE_CONTAINER: &str =
        r#"[{"Id":"c1","Names":["/web"],"Image":"nginx","State":"running","Status":"Up"}]"#;
    const EVENT: &str = r#"{"Type":"container","Action":"start","Actor":{"ID":"c1"}}"#;

    async fn woken(wake: &Notify) {
        tokio::time::timeout(Duration::from_secs(5), wake.notified())
            .await
            .expect("woken");
    }

    #[tokio::test]
    async fn events_trigger_refresh_and_end_of_stream_reconnects() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("docker.sock");
        let mut docker = FakeDocker::start(&sock);
        let fake = Fake::new(vec![instance("default", InstanceStatus::Running)], Ok(sock));
        let wake = Arc::new(Notify::new());
        let mut e = Engines::new(wake.clone(), Lang::En);

        let s = tick(&fake, &mut e).await.unwrap();
        assert_eq!(container_names(&s, "default"), Vec::<String>::new());
        let sub = docker.next_subscriber().await;

        // イベントが来るまでは取り直さない。
        *docker.containers.lock().unwrap() = ONE_CONTAINER.into();
        let s = tick(&fake, &mut e).await.unwrap();
        assert_eq!(container_names(&s, "default"), Vec::<String>::new());

        sub.send(Some(EVENT.into())).unwrap();
        woken(&wake).await;
        let s = tick(&fake, &mut e).await.unwrap();
        assert_eq!(container_names(&s, "default"), ["web"]);

        // 購読が切れたら、その周期は理由つきで使えないとし、接続を捨てる。
        sub.send(None).unwrap();
        woken(&wake).await;
        let s = tick(&fake, &mut e).await.unwrap();
        let reason = unavailable_reason(&s, "default");
        assert!(reason.contains("the docker event stream ended"), "{reason}");
        assert!(e.watches.is_empty());

        // 次の周期で接続し直す。
        let s = tick(&fake, &mut e).await.unwrap();
        assert_eq!(container_names(&s, "default"), ["web"]);
        let _sub = docker.next_subscriber().await;
        assert_eq!(e.watches.len(), 1);

        // 止まったインスタンスの接続は捨てる。
        fake.set_instances(Ok(vec![instance("default", InstanceStatus::Stopped)]));
        let s = tick(&fake, &mut e).await.unwrap();
        unavailable_reason(&s, "default");
        assert!(e.watches.is_empty());
    }

    fn fake_colima(dir: &tempfile::TempDir, body: &str) -> Colima {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.path().join("colima");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        Colima::new(p)
    }

    #[tokio::test]
    async fn colima_is_an_instance_source() {
        let d = tempfile::tempdir().unwrap();
        let c = fake_colima(
            &d,
            r#"case "$1" in
list) echo '{"name":"dev","status":"Stopped","arch":"aarch64","cpus":1,"memory":1,"disk":1,"runtime":"docker"}' ;;
status) echo '{"docker_socket":"unix:///s/dev.sock"}' ;;
esac"#,
        );
        let list = InstanceSource::list(&c).await.unwrap();
        assert_eq!(list[0].name, "dev");
        assert_eq!(
            InstanceSource::docker_socket(&c, "dev").await.unwrap(),
            PathBuf::from("/s/dev.sock")
        );
    }

    #[tokio::test]
    async fn action_success_wakes_monitor_without_error() {
        let d = tempfile::tempdir().unwrap();
        let c = fake_colima(&d, "echo starting");
        let wake = Arc::new(Notify::new());
        let r = run_action(
            c,
            vec!["start".into(), "dev".into()],
            wake.clone(),
            Lang::En,
        )
        .await;
        assert_eq!(r, None);
        woken(&wake).await;
    }

    #[tokio::test]
    async fn action_failure_keeps_last_lines_and_wakes_monitor() {
        let d = tempfile::tempdir().unwrap();
        let c = fake_colima(&d, "for i in 1 2 3 4 5 6 7; do echo line$i; done; exit 3");
        let wake = Arc::new(Notify::new());
        let r = run_action(c, vec!["stop".into(), "dev".into()], wake.clone(), Lang::En)
            .await
            .unwrap();
        assert!(r.contains("line3 / line4 / line5 / line6 / line7"), "{r}");
        assert!(!r.contains("line2"), "{r}");
        assert!(r.contains('3'), "{r}");
        woken(&wake).await;
    }

    #[tokio::test]
    async fn action_spawn_failure_is_reported() {
        let c = Colima::new(PathBuf::from("/nonexistent/colima"));
        let r = run_action(c, vec![], Arc::new(Notify::new()), Lang::En)
            .await
            .unwrap();
        assert!(r.contains("cannot run colima"), "{r}");
    }

    #[test]
    fn list_failure_carries_over_last_snapshot_with_new_time() {
        let old = Snapshot {
            updated_at: SystemTime::UNIX_EPOCH,
            instances: vec![instance("default", InstanceStatus::Running)],
            docker: BTreeMap::new(),
        };
        let s = carry_over(Some(old.clone()));
        assert_eq!(s.instances, old.instances);
        assert!(s.updated_at > SystemTime::UNIX_EPOCH);
        let empty = carry_over(None);
        assert!(empty.instances.is_empty() && empty.docker.is_empty());
        assert!(empty.updated_at > SystemTime::UNIX_EPOCH);
    }

    #[tokio::test]
    async fn vanished_instance_drops_its_engine() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("docker.sock");
        let mut docker = FakeDocker::start(&sock);
        let fake = Fake::new(vec![instance("default", InstanceStatus::Running)], Ok(sock));
        let mut e = engines();
        tick(&fake, &mut e).await.unwrap();
        let sub = docker.next_subscriber().await;
        assert_eq!(e.watches.len(), 1);
        fake.set_instances(Ok(vec![]));
        let s = tick(&fake, &mut e).await.unwrap();
        assert!(s.docker.is_empty());
        assert!(e.watches.is_empty());
        // 捨てた接続の購読タスクも止まり、偽物の側で接続が切れる。
        let closed = async {
            while sub.send(Some(EVENT.into())).is_ok() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(5), closed)
            .await
            .expect("event connection closed after the engine was dropped");
    }
}
