use super::*;
use crate::colima::{Arch, MountType, VmType};
use crate::docker::actions::{PortMap, Proto, RestartPolicy};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn fake_colima(dir: &tempfile::TempDir, body: &str) -> Colima {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.path().join("colima");
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    Colima::with_out_dir(p, dir.path().to_path_buf())
}

fn start(profile: &str) -> Request {
    Request::Colima {
        action: Action::Start,
        profile: profile.into(),
    }
}

fn no_terminal() -> OpenTerminal {
    Arc::new(|_, _| Ok(()))
}

fn recv(bridge: &Bridge) -> Reply {
    for _ in 0..200 {
        if let Some(r) = bridge.try_recv() {
            return r;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("no reply within 5 seconds");
}

/// `ColimaDone`が来るまでの返事を捨てて、`ColimaDone`の中身を返す。
fn recv_done(bridge: &Bridge) -> (String, Option<String>) {
    loop {
        if let Reply::ColimaDone { profile, error } = recv(bridge) {
            return (profile, error);
        }
    }
}

#[test]
fn pair_carries_requests_to_the_worker_and_replies_back() {
    let (bridge, mut ends) = Bridge::pair();
    assert_eq!(bridge.try_recv(), None);
    bridge.send(start("dev"));
    assert_eq!(ends.requests.try_recv().unwrap(), start("dev"));
    let done = Reply::ColimaDone {
        profile: "dev".into(),
        error: None,
    };
    ends.replies.send(done.clone()).unwrap();
    assert_eq!(bridge.try_recv(), Some(done));
}

#[test]
fn send_after_the_worker_stopped_does_not_panic() {
    let (bridge, ends) = Bridge::pair();
    drop(ends);
    bridge.send(start("dev"));
    assert_eq!(bridge.try_recv(), None);
}

#[test]
fn keys_do_not_collide_with_container_ids() {
    assert_eq!(run_key("dev"), "run:dev");
}

#[test]
fn worker_runs_colima_and_repaints_on_success() {
    let d = tempfile::tempdir().unwrap();
    let log = d.path().join("args");
    let colima = fake_colima(&d, &format!("echo \"$@\" > '{}'", log.display()));
    let repaints = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = repaints.clone();
    let bridge = Bridge::spawn(
        colima,
        Lang::En,
        move || {
            counter.fetch_add(1, Ordering::SeqCst);
        },
        no_terminal(),
    )
    .unwrap();
    bridge.send(start("dev"));
    assert_eq!(recv_done(&bridge), ("dev".to_string(), None));
    assert_eq!(std::fs::read_to_string(&log).unwrap().trim(), "start dev");
    // 成功の知らせと終わりの2つ。
    assert_eq!(repaints.load(Ordering::SeqCst), 2);
}

/// `ColimaDone`までの返事をすべて集める。
fn replies_until_colima_done(bridge: &Bridge) -> Vec<Reply> {
    let mut all = Vec::new();
    loop {
        let r = recv(bridge);
        let done = matches!(r, Reply::ColimaDone { .. });
        all.push(r);
        if done {
            return all;
        }
    }
}

#[test]
fn a_successful_colima_action_is_announced_in_the_language() {
    for (lang, want) in [
        (Lang::En, "dev: colima start finished"),
        (Lang::Ja, "dev: colima start が終わりました"),
    ] {
        let d = tempfile::tempdir().unwrap();
        let bridge = Bridge::spawn(fake_colima(&d, "true"), lang, || {}, no_terminal()).unwrap();
        bridge.send(start("dev"));
        let all = replies_until_colima_done(&bridge);
        assert_eq!(
            all,
            [
                Reply::Notice(want.into()),
                Reply::ColimaDone {
                    profile: "dev".into(),
                    error: None
                }
            ]
        );
    }
    let d = tempfile::tempdir().unwrap();
    let bridge = Bridge::spawn(fake_colima(&d, "exit 3"), Lang::En, || {}, no_terminal()).unwrap();
    bridge.send(start("dev"));
    let all = replies_until_colima_done(&bridge);
    assert!(
        !all.iter().any(|r| matches!(r, Reply::Notice(_))),
        "{all:?}"
    );
}

#[test]
fn worker_streams_colima_output_lines_before_done() {
    let d = tempfile::tempdir().unwrap();
    let colima = fake_colima(&d, "echo one; echo two");
    let bridge = Bridge::spawn(colima, Lang::En, || {}, no_terminal()).unwrap();
    bridge.send(start("dev"));
    let line = |s: &str| Reply::InstanceLine {
        profile: "dev".into(),
        line: s.into(),
    };
    let mut lines = vec![recv(&bridge), recv(&bridge)];
    lines.sort_by_key(|r| format!("{r:?}"));
    assert_eq!(lines, vec![line("one"), line("two")]);
    assert_eq!(recv_done(&bridge), ("dev".to_string(), None));
}

#[test]
fn worker_reports_a_failing_colima_with_its_output() {
    let d = tempfile::tempdir().unwrap();
    let colima = fake_colima(&d, "echo boom; exit 3");
    let bridge = Bridge::spawn(colima, Lang::En, || {}, no_terminal()).unwrap();
    bridge.send(Request::Colima {
        action: Action::Delete,
        profile: "dev".into(),
    });
    let (profile, error) = recv_done(&bridge);
    assert_eq!(profile, "dev");
    assert!(error.unwrap().contains("boom"));
}

#[test]
fn worker_rejects_an_unsafe_profile_without_running_colima() {
    let d = tempfile::tempdir().unwrap();
    let ran = d.path().join("ran");
    let colima = fake_colima(&d, &format!("touch '{}'", ran.display()));
    let bridge = Bridge::spawn(colima, Lang::Ja, || {}, no_terminal()).unwrap();
    bridge.send(start("a b"));
    assert!(
        recv_done(&bridge)
            .1
            .unwrap()
            .contains("プロフィール名が不正")
    );
    assert!(!ran.exists());
}

#[test]
fn worker_reports_a_missing_colima_binary() {
    let d = tempfile::tempdir().unwrap();
    let colima = Colima::with_out_dir(PathBuf::from("/nonexistent/colima"), d.path().to_path_buf());
    let bridge = Bridge::spawn(colima, Lang::En, || {}, no_terminal()).unwrap();
    bridge.send(start("dev"));
    assert!(recv_done(&bridge).1.unwrap().contains("cannot run colima"));
}

fn create_spec(profile: &str) -> CreateSpec {
    CreateSpec {
        profile: profile.into(),
        cpus: 2,
        memory_gib: 4.0,
        disk_gib: 60,
        vm_type: VmType::Vz,
        arch: Arch::Aarch64,
        mount_type: MountType::Virtiofs,
    }
}

#[test]
fn create_runs_colima_start_with_the_form_values() {
    let d = tempfile::tempdir().unwrap();
    let log = d.path().join("args");
    let colima = fake_colima(&d, &format!("echo \"$@\" > '{}'", log.display()));
    let bridge = Bridge::spawn(colima, Lang::En, || {}, no_terminal()).unwrap();
    let spec = create_spec("fresh");
    bridge.send(Request::Create(spec.clone()));
    assert_eq!(recv_done(&bridge), ("fresh".to_string(), None));
    let want = colima::start_args(&spec).unwrap().join(" ");
    assert_eq!(std::fs::read_to_string(&log).unwrap().trim(), want);
}

#[test]
fn create_with_a_bad_profile_reports_it_without_running_colima() {
    let d = tempfile::tempdir().unwrap();
    let ran = d.path().join("ran");
    let colima = fake_colima(&d, &format!("touch '{}'", ran.display()));
    let bridge = Bridge::spawn(colima, Lang::En, || {}, no_terminal()).unwrap();
    bridge.send(Request::Create(create_spec("a b")));
    let (profile, error) = recv_done(&bridge);
    assert_eq!(profile, "a b");
    assert!(error.is_some());
    assert!(!ran.exists());
}

// --- 偽のdockerソケット ---

enum Body {
    Full(&'static str),
    /// 先頭のチャンクを返したあと、相手が切るまで閉じない。
    Hang(Vec<u8>),
}

struct Route {
    contains: &'static str,
    status: u16,
    body: Body,
}

fn full(contains: &'static str, status: u16, body: &'static str) -> Route {
    Route {
        contains,
        status,
        body: Body::Full(body),
    }
}

struct Env {
    _dir: tempfile::TempDir,
    bridge: Bridge,
    seen: Arc<Mutex<Vec<String>>>,
    opened: Arc<Mutex<Vec<ShellCommand>>>,
    hang_closed: Arc<AtomicBool>,
    colima_bin: PathBuf,
    socket: PathBuf,
}

fn serve_connection(
    conn: std::os::unix::net::UnixStream,
    routes: Arc<Vec<Route>>,
    seen: Arc<Mutex<Vec<String>>>,
    hang_closed: Arc<AtomicBool>,
) {
    let mut reader = BufReader::new(conn.try_clone().unwrap());
    let mut conn = conn;
    loop {
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
            return;
        }
        let mut length = 0usize;
        loop {
            let mut h = String::new();
            reader.read_line(&mut h).unwrap();
            if h == "\r\n" {
                break;
            }
            if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().unwrap();
            }
        }
        let mut body = vec![0; length];
        reader.read_exact(&mut body).unwrap();
        let mut parts = request_line.split_whitespace();
        let (method, path) = (parts.next().unwrap(), parts.next().unwrap());
        seen.lock().unwrap().push(format!("{method} {path}"));
        let Some(route) = routes.iter().find(|r| path.contains(r.contains)) else {
            conn.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            continue;
        };
        match &route.body {
            Body::Full(text) => {
                let head = format!(
                    "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{text}",
                    route.status,
                    text.len()
                );
                conn.write_all(head.as_bytes()).unwrap();
            }
            Body::Hang(chunk) => {
                conn.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n")
                    .unwrap();
                conn.write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                    .unwrap();
                conn.write_all(chunk).unwrap();
                conn.write_all(b"\r\n").unwrap();
                // 相手が切ると読みが0で返る。
                let mut rest = Vec::new();
                let _ = reader.read_to_end(&mut rest);
                hang_closed.store(true, Ordering::SeqCst);
                return;
            }
        }
    }
}

/// 1行のログを、dockerの多重化の枠にする。
fn log_frame(text: &str) -> Vec<u8> {
    let mut v = vec![1, 0, 0, 0];
    v.extend((text.len() as u32).to_be_bytes());
    v.extend(text.as_bytes());
    v
}

fn env(routes: Vec<Route>, status_ok: bool) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("docker.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let hang_closed = Arc::new(AtomicBool::new(false));
    let routes = Arc::new(routes);
    let (s, h) = (seen.clone(), hang_closed.clone());
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let (r, s, h) = (routes.clone(), s.clone(), h.clone());
            std::thread::spawn(move || serve_connection(conn.unwrap(), r, s, h));
        }
    });
    let status = if status_ok {
        format!(
            "echo '{{\"docker_socket\":\"unix://{}\"}}'",
            socket.display()
        )
    } else {
        "echo '{\"docker_socket\":\"tcp://nowhere\"}'".to_string()
    };
    let colima = fake_colima(&dir, &format!("[ \"$1\" = status ] && {status}"));
    let colima_bin = colima.bin().to_path_buf();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let log = opened.clone();
    let terminal: OpenTerminal = Arc::new(move |argv, env| {
        log.lock().unwrap().push((argv.to_vec(), env.to_vec()));
        Ok(())
    });
    let bridge = Bridge::spawn(colima, Lang::En, || {}, terminal).unwrap();
    Env {
        _dir: dir,
        bridge,
        seen,
        opened,
        hang_closed,
        colima_bin,
        socket,
    }
}

fn container(profile: &str, id: &str, op: ContainerOp) -> Request {
    Request::Container {
        profile: profile.into(),
        id: id.into(),
        op,
    }
}

fn op_done(env: &Env) -> (String, Option<String>) {
    match recv(&env.bridge) {
        Reply::OpDone { key, error } => (key, error),
        other => panic!("{other:?}"),
    }
}

fn seen(env: &Env) -> Vec<String> {
    env.seen.lock().unwrap().clone()
}

fn has_call(env: &Env, method: &str, path_part: &str) -> bool {
    seen(env)
        .iter()
        .any(|l| l.starts_with(method) && l.contains(path_part))
}

#[test]
fn container_ops_call_docker_and_reply_once() {
    let cases = [
        (ContainerOp::Start, "/containers/abc1/start"),
        (ContainerOp::Stop, "/containers/abc1/stop"),
        (ContainerOp::Restart, "/containers/abc1/restart"),
        (ContainerOp::Pause, "/containers/abc1/pause"),
        (ContainerOp::Unpause, "/containers/abc1/unpause"),
    ];
    for (op, path) in cases {
        let e = env(vec![full("/containers/abc1/", 204, "")], true);
        e.bridge.send(container("dev", "abc1", op));
        assert_eq!(op_done(&e), ("abc1".to_string(), None), "{op:?}");
        assert!(has_call(&e, "POST", path), "{op:?}: {:?}", seen(&e));
        assert_eq!(e.bridge.try_recv(), None, "{op:?}");
    }
}

#[test]
fn remove_passes_the_force_flag() {
    for (force, want) in [(true, "force=true"), (false, "force=false")] {
        let e = env(vec![full("/containers/abc1", 204, "")], true);
        e.bridge
            .send(container("dev", "abc1", ContainerOp::Remove { force }));
        assert_eq!(recv_notice_then_done(&e).1, "abc1");
        assert!(has_call(&e, "DELETE", want), "{:?}", seen(&e));
    }
}

#[test]
fn a_docker_error_comes_back_as_the_reply_error() {
    let e = env(
        vec![full("/containers/abc1/stop", 500, "{\"message\":\"nope\"}")],
        true,
    );
    e.bridge.send(container("dev", "abc1", ContainerOp::Stop));
    let (key, error) = op_done(&e);
    assert_eq!(key, "abc1");
    assert!(error.unwrap().contains("nope"));
}

#[test]
fn a_socket_lookup_failure_still_gets_one_reply() {
    let e = env(vec![], false);
    e.bridge.send(container("dev", "abc1", ContainerOp::Stop));
    let (key, error) = op_done(&e);
    assert_eq!(key, "abc1");
    assert!(error.unwrap().contains("unexpected docker socket address"));
    assert!(seen(&e).is_empty());
}

#[test]
fn an_unsafe_container_id_is_refused_before_docker_is_called() {
    let e = env(vec![full("/", 204, "")], true);
    e.bridge.send(container("dev", "x/y", ContainerOp::Stop));
    assert!(op_done(&e).1.is_some());
    assert!(seen(&e).is_empty());
}

#[test]
fn rename_posts_the_new_name_and_replies_under_the_container_id() {
    let e = env(vec![full("/containers/c1/rename", 204, "")], true);
    e.bridge.send(Request::Rename {
        profile: "dev".into(),
        id: "c1".into(),
        name: "web-2".into(),
    });
    assert_eq!(op_done(&e), ("c1".into(), None));
    assert!(has_call(&e, "POST", "name=web-2"), "{:?}", seen(&e));
    let e = env(vec![full("/", 204, "")], true);
    e.bridge.send(Request::Rename {
        profile: "dev".into(),
        id: "c1".into(),
        name: "a/b".into(),
    });
    assert!(op_done(&e).1.is_some());
    assert!(seen(&e).is_empty());
}

fn run_spec() -> RunSpec {
    RunSpec {
        image: "alpine:latest".into(),
        name: Some("wk-run".into()),
        ports: vec![PortMap {
            host: 8080,
            container: 80,
            proto: Proto::Tcp,
        }],
        env: vec![],
        mounts: vec![],
        restart: RestartPolicy::No,
        cmd: None,
    }
}

#[test]
fn run_creates_and_starts_then_replies_with_the_run_key() {
    let e = env(
        vec![
            full(
                "/containers/create",
                201,
                "{\"Id\":\"newid1\",\"Warnings\":[]}",
            ),
            full("/containers/newid1/start", 204, ""),
        ],
        true,
    );
    e.bridge.send(Request::Run {
        profile: "dev".into(),
        spec: run_spec(),
    });
    assert_eq!(op_done(&e), (run_key("dev"), None));
    assert!(has_call(&e, "POST", "/containers/create"));
    assert!(has_call(&e, "POST", "/containers/newid1/start"));
}

#[test]
fn run_failure_is_reported_under_the_run_key() {
    let e = env(
        vec![full(
            "/containers/create",
            404,
            "{\"message\":\"no such image\"}",
        )],
        true,
    );
    e.bridge.send(Request::Run {
        profile: "dev".into(),
        spec: run_spec(),
    });
    let (key, error) = op_done(&e);
    assert_eq!(key, run_key("dev"));
    assert!(error.unwrap().contains("no such image"));
}

#[test]
fn inspect_returns_pretty_json() {
    let e = env(
        vec![full("/containers/abc1/json", 200, "{\"Id\":\"abc1\"}")],
        true,
    );
    e.bridge.send(Request::Inspect {
        profile: "dev".into(),
        id: "abc1".into(),
    });
    let Reply::Inspected { id, text } = recv(&e.bridge) else {
        panic!("not inspected");
    };
    assert_eq!(id, "abc1");
    assert!(text.contains("\"Id\": \"abc1\""), "{text}");
}

#[test]
fn inspect_failure_is_a_failed_reply() {
    let e = env(vec![full("/json", 500, "{\"message\":\"gone\"}")], true);
    e.bridge.send(Request::Inspect {
        profile: "dev".into(),
        id: "abc1".into(),
    });
    let Reply::Failed(m) = recv(&e.bridge) else {
        panic!("not failed");
    };
    assert!(m.contains("gone"));
}

#[test]
fn usage_returns_cpu_and_memory() {
    let stats = "{\"cpu_stats\":{\"cpu_usage\":{\"total_usage\":200},\"system_cpu_usage\":1000,\"online_cpus\":2},\"precpu_stats\":{\"cpu_usage\":{\"total_usage\":100},\"system_cpu_usage\":500,\"online_cpus\":2},\"memory_stats\":{\"usage\":3000,\"limit\":4096,\"stats\":{\"inactive_file\":1000}}}";
    let e = env(vec![full("/containers/abc1/stats", 200, stats)], true);
    e.bridge.send(Request::Usage {
        profile: "dev".into(),
        id: "abc1".into(),
    });
    let Reply::UsageSample { id, usage } = recv(&e.bridge) else {
        panic!("no usage");
    };
    assert_eq!(id, "abc1");
    assert_eq!(usage.mem_bytes, 2000);
    assert_eq!(usage.mem_limit, 4096);
    assert!(
        (usage.cpu_percent - 40.0).abs() < 0.01,
        "{}",
        usage.cpu_percent
    );
}

#[test]
fn usage_failure_is_a_failed_reply() {
    let e = env(vec![], false);
    e.bridge.send(Request::Usage {
        profile: "dev".into(),
        id: "abc1".into(),
    });
    assert!(matches!(recv(&e.bridge), Reply::Failed(_)));
}

#[test]
fn logs_arrive_line_by_line() {
    let mut body = log_frame("hello\n");
    body.extend(log_frame("world\n"));
    let e = env(
        vec![Route {
            contains: "/containers/abc1/logs",
            status: 200,
            body: Body::Hang(body),
        }],
        true,
    );
    e.bridge.send(Request::Logs {
        profile: "dev".into(),
        id: "abc1".into(),
        follow: true,
    });
    let line = |s: &str| Reply::LogLine {
        id: "abc1".into(),
        line: s.into(),
    };
    assert_eq!(recv(&e.bridge), line("hello"));
    assert_eq!(recv(&e.bridge), line("world"));
    assert!(has_call(&e, "GET", "follow=true"), "{:?}", seen(&e));
}

#[test]
fn stop_logs_closes_the_stream() {
    let e = env(
        vec![Route {
            contains: "/logs",
            status: 200,
            body: Body::Hang(log_frame("x\n")),
        }],
        true,
    );
    e.bridge.send(Request::Logs {
        profile: "dev".into(),
        id: "abc1".into(),
        follow: true,
    });
    assert!(matches!(recv(&e.bridge), Reply::LogLine { .. }));
    assert!(!e.hang_closed.load(Ordering::SeqCst));
    e.bridge.send(Request::StopLogs { id: "abc1".into() });
    for _ in 0..200 {
        if e.hang_closed.load(Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("the log stream stayed open");
}

#[test]
fn a_new_logs_request_replaces_the_old_stream_of_the_same_container() {
    let e = env(
        vec![Route {
            contains: "/logs",
            status: 200,
            body: Body::Hang(log_frame("x\n")),
        }],
        true,
    );
    let logs = |follow| Request::Logs {
        profile: "dev".into(),
        id: "abc1".into(),
        follow,
    };
    e.bridge.send(logs(true));
    assert!(matches!(recv(&e.bridge), Reply::LogLine { .. }));
    e.bridge.send(logs(false));
    for _ in 0..200 {
        if e.hang_closed.load(Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("the first stream was not replaced");
}

#[test]
fn logs_failure_is_a_failed_reply() {
    let e = env(vec![], false);
    e.bridge.send(Request::Logs {
        profile: "dev".into(),
        id: "abc1".into(),
        follow: false,
    });
    assert!(matches!(recv(&e.bridge), Reply::Failed(_)));
}

#[test]
fn logs_of_a_failing_daemon_report_the_api_error() {
    let e = env(vec![full("/logs", 500, "{\"message\":\"no logs\"}")], true);
    e.bridge.send(Request::Logs {
        profile: "dev".into(),
        id: "abc1".into(),
        follow: false,
    });
    let Reply::Failed(m) = recv(&e.bridge) else {
        panic!("not failed");
    };
    assert!(m.contains("no logs"), "{m}");
}

fn opened(env: &Env) -> Vec<ShellCommand> {
    for _ in 0..200 {
        let v = env.opened.lock().unwrap().clone();
        if !v.is_empty() {
            return v;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    vec![]
}

#[test]
fn instance_shell_opens_colima_ssh() {
    let e = env(vec![], true);
    e.bridge.send(Request::Shell {
        profile: "dev".into(),
        container: None,
    });
    let want = vec![
        e.colima_bin.display().to_string(),
        "ssh".into(),
        "-p".into(),
        "dev".into(),
    ];
    assert_eq!(opened(&e), vec![(want, vec![])]);
}

#[test]
fn container_shell_opens_docker_exec_with_docker_host() {
    let e = env(vec![], true);
    e.bridge.send(Request::Shell {
        profile: "dev".into(),
        container: Some("abc1".into()),
    });
    let calls = opened(&e);
    assert_eq!(calls.len(), 1);
    let (argv, env_vars) = &calls[0];
    assert_eq!(argv[1..], ["exec", "-it", "abc1", "sh"]);
    assert!(argv[0].ends_with("docker"), "{argv:?}");
    assert_eq!(
        env_vars,
        &vec![(
            "DOCKER_HOST".to_string(),
            format!("unix://{}", e.socket.display())
        )]
    );
}

#[test]
fn shell_with_an_unsafe_target_fails_without_opening_a_terminal() {
    let e = env(vec![], true);
    e.bridge.send(Request::Shell {
        profile: "dev".into(),
        container: Some("x/y".into()),
    });
    assert!(matches!(recv(&e.bridge), Reply::Failed(_)));
    e.bridge.send(Request::Shell {
        profile: "a b".into(),
        container: None,
    });
    assert!(matches!(recv(&e.bridge), Reply::Failed(_)));
    assert!(e.opened.lock().unwrap().is_empty());
}

#[test]
fn a_container_shell_for_an_instance_without_a_socket_fails() {
    let e = env(vec![], false);
    e.bridge.send(Request::Shell {
        profile: "dev".into(),
        container: Some("abc1".into()),
    });
    assert!(matches!(recv(&e.bridge), Reply::Failed(_)));
}

#[test]
fn a_terminal_that_cannot_open_is_reported() {
    let d = tempfile::tempdir().unwrap();
    let colima = fake_colima(&d, "true");
    let terminal: OpenTerminal = Arc::new(|_, _| Err(io::Error::other("no terminal")));
    let bridge = Bridge::spawn(colima, Lang::En, || {}, terminal).unwrap();
    bridge.send(Request::Shell {
        profile: "dev".into(),
        container: None,
    });
    let Reply::Failed(m) = recv(&bridge) else {
        panic!("not failed");
    };
    assert!(
        m.contains("cannot open the terminal") && m.contains("no terminal"),
        "{m}"
    );
}

#[test]
fn container_shell_builds_docker_exec_with_the_socket() {
    let (argv, env_vars) = container_shell(
        Path::new("/opt/homebrew/bin/docker"),
        Path::new("/Users/x/.colima/dev/docker.sock"),
        "web-1",
    )
    .unwrap();
    assert_eq!(
        argv,
        ["/opt/homebrew/bin/docker", "exec", "-it", "web-1", "sh"]
    );
    assert_eq!(
        env_vars,
        [(
            "DOCKER_HOST".to_string(),
            "unix:///Users/x/.colima/dev/docker.sock".to_string()
        )]
    );
    assert_eq!(
        container_shell(Path::new("docker"), Path::new("/s"), "-x;rm").unwrap_err(),
        DockerError::InvalidId("-x;rm".into())
    );
}

// --- イメージ、ボリューム、ネットワーク ---

fn recv_notice_then_done(env: &Env) -> (String, String, Option<String>) {
    let Reply::Notice(text) = recv(&env.bridge) else {
        panic!("expected a notice first");
    };
    let (key, error) = op_done(env);
    (text, key, error)
}

fn remove_image(force: bool) -> Request {
    Request::RemoveImage {
        profile: "dev".into(),
        id: "nginx:alpine".into(),
        force,
        size: 2048,
    }
}

#[test]
fn remove_image_deletes_with_the_force_flag_and_replies_under_the_image_key() {
    const DELETED: &str = r#"[{"Untagged":"nginx:alpine"},{"Deleted":"sha256:aa"}]"#;
    for (force, want) in [(true, "force=true"), (false, "force=false")] {
        let e = env(vec![full("/images/nginx:alpine", 200, DELETED)], true);
        e.bridge.send(remove_image(force));
        // 本体まで消えたら、覚えておいた大きさを空いた量として知らせる。
        assert_eq!(
            recv_notice_then_done(&e),
            ("Freed 2.0 KiB".into(), image_key("nginx:alpine"), None)
        );
        assert!(has_call(&e, "DELETE", want), "{:?}", seen(&e));
        assert_eq!(e.bridge.try_recv(), None);
    }
}

#[test]
fn removing_one_of_several_tags_frees_nothing() {
    let e = env(
        vec![full(
            "/images/nginx:alpine",
            200,
            r#"[{"Untagged":"nginx:alpine"}]"#,
        )],
        true,
    );
    e.bridge.send(remove_image(false));
    assert_eq!(
        recv_notice_then_done(&e),
        (
            "Removed tag nginx:alpine".into(),
            image_key("nginx:alpine"),
            None
        )
    );
}

#[test]
fn a_failed_delete_sends_no_notice() {
    let e = env(
        vec![full(
            "/images/nginx:alpine",
            409,
            "{\"message\":\"in use\"}",
        )],
        true,
    );
    e.bridge.send(remove_image(false));
    let (key, error) = op_done(&e);
    assert_eq!(key, image_key("nginx:alpine"));
    assert!(error.unwrap().contains("in use"));
    assert_eq!(e.bridge.try_recv(), None);
}

#[test]
fn removing_a_volume_or_container_is_announced() {
    for (size, want) in [(Some(10), "Freed 10 B"), (None, "Deleted data-1")] {
        let e = env(vec![full("/volumes/data-1", 204, "")], true);
        e.bridge.send(Request::RemoveVolume {
            profile: "dev".into(),
            name: "data-1".into(),
            size,
        });
        assert_eq!(
            recv_notice_then_done(&e),
            (want.into(), volume_key("data-1"), None)
        );
    }
    let e = env(vec![full("/containers/c1", 204, "")], true);
    e.bridge
        .send(container("dev", "c1", ContainerOp::Remove { force: true }));
    assert_eq!(
        recv_notice_then_done(&e),
        ("Deleted the container".into(), "c1".into(), None)
    );
    // 止めるだけでは知らせない。
    let e = env(vec![full("/containers/c1/stop", 204, "")], true);
    e.bridge.send(container("dev", "c1", ContainerOp::Stop));
    assert_eq!(op_done(&e), ("c1".into(), None));
    assert_eq!(e.bridge.try_recv(), None);
}

#[test]
fn remove_volume_and_network_reply_under_their_keys() {
    let e = env(vec![full("/volumes/data-1", 204, "")], true);
    e.bridge.send(Request::RemoveVolume {
        profile: "dev".into(),
        name: "data-1".into(),
        size: None,
    });
    assert_eq!(recv_notice_then_done(&e).1, volume_key("data-1"));
    assert!(has_call(&e, "DELETE", "/volumes/data-1"));

    let e = env(vec![full("/networks/abc123", 204, "")], true);
    e.bridge.send(Request::RemoveNetwork {
        profile: "dev".into(),
        id: "abc123".into(),
    });
    assert_eq!(op_done(&e), (network_key("abc123"), None));
    assert!(has_call(&e, "DELETE", "/networks/abc123"));
}

#[test]
fn a_busy_volume_reports_dockers_reason() {
    let e = env(
        vec![full(
            "/volumes/data-1",
            409,
            "{\"message\":\"volume is in use\"}",
        )],
        true,
    );
    e.bridge.send(Request::RemoveVolume {
        profile: "dev".into(),
        name: "data-1".into(),
        size: None,
    });
    let (key, error) = op_done(&e);
    assert_eq!(key, volume_key("data-1"));
    assert!(error.unwrap().contains("volume is in use"));
}

#[test]
fn unsafe_resource_names_are_refused_before_docker_is_called() {
    let e = env(vec![full("/", 204, "")], true);
    e.bridge.send(Request::RemoveVolume {
        profile: "dev".into(),
        name: "x/y".into(),
        size: None,
    });
    assert!(op_done(&e).1.is_some());
    e.bridge.send(Request::RemoveNetwork {
        profile: "dev".into(),
        id: "my net".into(),
    });
    assert!(op_done(&e).1.is_some());
    assert!(seen(&e).is_empty());
}

#[test]
fn prune_images_tells_how_much_was_freed_then_finishes() {
    let e = env(
        vec![full("/images/prune", 200, "{\"SpaceReclaimed\":1048576}")],
        true,
    );
    e.bridge.send(Request::PruneImages {
        profile: "dev".into(),
        dangling: true,
    });
    assert_eq!(
        recv_notice_then_done(&e),
        (
            "Freed 1.0 MiB".to_string(),
            Cleanup::Images.key("dev"),
            None
        )
    );
    assert!(has_call(&e, "POST", "%22true%22"), "{:?}", seen(&e));
}

#[test]
fn prune_build_cache_volumes_and_networks_report_their_results() {
    let e = env(
        vec![full("/build/prune", 200, "{\"SpaceReclaimed\":2048}")],
        true,
    );
    e.bridge.send(Request::PruneBuildCache {
        profile: "dev".into(),
    });
    assert_eq!(
        recv_notice_then_done(&e),
        (
            "Freed 2.0 KiB".to_string(),
            Cleanup::BuildCache.key("dev"),
            None
        )
    );

    let e = env(
        vec![full("/volumes/prune", 200, "{\"SpaceReclaimed\":0}")],
        true,
    );
    e.bridge.send(Request::PruneVolumes {
        profile: "dev".into(),
    });
    assert_eq!(
        recv_notice_then_done(&e),
        ("Freed 0 B".to_string(), Cleanup::Volumes.key("dev"), None)
    );

    let e = env(
        vec![full(
            "/networks/prune",
            200,
            "{\"NetworksDeleted\":[\"a\",\"b\"]}",
        )],
        true,
    );
    e.bridge.send(Request::PruneNetworks {
        profile: "dev".into(),
    });
    assert_eq!(
        recv_notice_then_done(&e),
        (
            "Removed 2 networks".to_string(),
            Cleanup::Networks.key("dev"),
            None
        )
    );
}

#[test]
fn a_failed_prune_replies_with_the_error_and_no_notice() {
    let e = env(
        vec![full("/volumes/prune", 500, "{\"message\":\"nope\"}")],
        true,
    );
    e.bridge.send(Request::PruneVolumes {
        profile: "dev".into(),
    });
    let (key, error) = op_done(&e);
    assert_eq!(key, Cleanup::Volumes.key("dev"));
    assert!(error.unwrap().contains("nope"));
    assert_eq!(e.bridge.try_recv(), None);
}

#[test]
fn reclaimed_text_is_localized() {
    assert_eq!(
        reclaimed_text(&Reclaimed::Bytes(1536), Lang::En),
        "Freed 1.5 KiB"
    );
    assert_eq!(
        reclaimed_text(&Reclaimed::Bytes(1536), Lang::Ja),
        "1.5 KiB を空けました"
    );
    assert_eq!(
        reclaimed_text(&Reclaimed::Networks(3), Lang::En),
        "Removed 3 networks"
    );
    assert_eq!(
        reclaimed_text(&Reclaimed::Networks(3), Lang::Ja),
        "ネットワークを 3 件削除しました"
    );
}

#[test]
fn create_volume_and_network_post_to_docker() {
    let e = env(
        vec![full(
            "/volumes/create",
            201,
            "{\"Name\":\"data-1\",\"Driver\":\"local\",\"Mountpoint\":\"/m\",\"Labels\":{},\"Options\":{},\"Scope\":\"local\"}",
        )],
        true,
    );
    e.bridge.send(Request::CreateVolume {
        profile: "dev".into(),
        name: "data-1".into(),
    });
    assert_eq!(op_done(&e), (create_volume_key("dev"), None));
    assert!(has_call(&e, "POST", "/volumes/create"));

    let e = env(
        vec![full(
            "/networks/create",
            201,
            "{\"Id\":\"n1\",\"Warning\":\"\"}",
        )],
        true,
    );
    e.bridge.send(Request::CreateNetwork {
        profile: "dev".into(),
        name: "my net".into(),
        driver: NetDriver::Macvlan,
    });
    assert_eq!(op_done(&e), (create_network_key("dev"), None));
    assert!(has_call(&e, "POST", "/networks/create"));
}

#[test]
fn pull_streams_progress_then_finishes() {
    let e = env(
        vec![full(
            "/images/create",
            200,
            "{\"status\":\"Downloading\",\"id\":\"l1\",\"progressDetail\":{\"current\":5,\"total\":10}}\n{\"status\":\"Pull complete\",\"id\":\"l1\"}\n",
        )],
        true,
    );
    e.bridge.send(Request::Pull {
        profile: "dev".into(),
        reference: "nginx:alpine".into(),
    });
    let progress = |p: PullProgress| Reply::PullProgress {
        profile: "dev".into(),
        progress: p,
    };
    assert_eq!(
        recv(&e.bridge),
        progress(PullProgress {
            id: Some("l1".into()),
            status: "Downloading".into(),
            current: Some(5),
            total: Some(10)
        })
    );
    assert_eq!(
        recv(&e.bridge),
        progress(PullProgress {
            id: Some("l1".into()),
            status: "Pull complete".into(),
            current: None,
            total: None
        })
    );
    assert_eq!(op_done(&e), (pull_key("dev"), None));
    assert!(
        has_call(&e, "POST", "fromImage=nginx%3Aalpine")
            || has_call(&e, "POST", "fromImage=nginx:alpine"),
        "{:?}",
        seen(&e)
    );
}

#[test]
fn a_pull_error_in_the_stream_is_the_reply_error() {
    let e = env(
        vec![full(
            "/images/create",
            200,
            "{\"errorDetail\":{\"message\":\"denied\"},\"error\":\"denied\"}\n",
        )],
        true,
    );
    e.bridge.send(Request::Pull {
        profile: "dev".into(),
        reference: "nginx:alpine".into(),
    });
    let (key, error) = op_done(&e);
    assert_eq!(key, pull_key("dev"));
    assert!(error.unwrap().contains("denied"));
}

#[test]
fn a_pull_without_a_docker_socket_still_gets_one_reply() {
    let e = env(vec![], false);
    e.bridge.send(Request::Pull {
        profile: "dev".into(),
        reference: "nginx:alpine".into(),
    });
    let (key, error) = op_done(&e);
    assert_eq!(key, pull_key("dev"));
    assert!(error.unwrap().contains("unexpected docker socket address"));
    assert_eq!(e.bridge.try_recv(), None);
}

#[test]
fn tag_image_posts_repo_and_tag_and_replies_under_the_tag_key() {
    let e = env(vec![full("/images/nginx:alpine/tag", 201, "")], true);
    e.bridge.send(Request::TagImage {
        profile: "dev".into(),
        id: "nginx:alpine".into(),
        repo: "ghcr.io/o/app".into(),
        tag: "v1".into(),
    });
    assert_eq!(op_done(&e), (tag_key("dev"), None));
    assert!(
        has_call(&e, "POST", "repo=ghcr.io%2Fo%2Fapp"),
        "{:?}",
        seen(&e)
    );
    assert!(has_call(&e, "POST", "tag=v1"));
}

#[test]
fn a_bad_tag_is_refused_before_docker_is_called() {
    let e = env(vec![full("/", 201, "")], true);
    e.bridge.send(Request::TagImage {
        profile: "dev".into(),
        id: "nginx:alpine".into(),
        repo: "a b".into(),
        tag: "v1".into(),
    });
    assert!(op_done(&e).1.is_some());
    assert!(seen(&e).is_empty());
}

#[test]
fn inspect_resource_returns_pretty_json_for_each_kind() {
    let cases = [
        (
            ResourceKind::Image,
            "nginx:alpine",
            "/images/nginx:alpine/json",
            "{\"Id\":\"sha256:aa\"}",
        ),
        (
            ResourceKind::Volume,
            "data-1",
            "/volumes/data-1",
            "{\"Name\":\"data-1\",\"Driver\":\"local\",\"Mountpoint\":\"/m\",\"Labels\":{},\"Options\":{},\"Scope\":\"local\"}",
        ),
        (
            ResourceKind::Network,
            "abc123",
            "/networks/abc123",
            "{\"Name\":\"n\"}",
        ),
    ];
    for (kind, target, route, body) in cases {
        let e = env(vec![full(route, 200, body)], true);
        e.bridge.send(Request::InspectResource {
            profile: "dev".into(),
            kind,
            target: target.into(),
        });
        match recv(&e.bridge) {
            Reply::ResourceInspected {
                kind: k,
                target: t,
                text,
            } => {
                assert_eq!((k, t.as_str()), (kind, target));
                assert!(
                    text.contains('\n') && text.contains("Name") || text.contains("Id"),
                    "{text}"
                );
            }
            other => panic!("{kind:?}: {other:?}"),
        }
        assert!(has_call(&e, "GET", route), "{:?}", seen(&e));
    }
}

#[test]
fn inspect_resource_failure_is_a_failed_reply() {
    let e = env(
        vec![full(
            "/volumes/data-1",
            404,
            "{\"message\":\"no such volume\"}",
        )],
        true,
    );
    e.bridge.send(Request::InspectResource {
        profile: "dev".into(),
        kind: ResourceKind::Volume,
        target: "data-1".into(),
    });
    match recv(&e.bridge) {
        Reply::Failed(m) => assert!(m.contains("no such volume")),
        other => panic!("{other:?}"),
    }
}
