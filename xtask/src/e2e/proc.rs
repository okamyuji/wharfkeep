//! 子プロセスの補助。
use super::*;

/// 成功したときの標準出力。
pub(super) fn text(cmd: &mut Command) -> Option<String> {
    let out = cmd
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 終了の状態によらない標準出力。pgrepは見つからないと1で終わる。
pub(super) fn stdout(cmd: &mut Command) -> String {
    cmd.stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

pub(super) fn ok(cmd: &mut Command) -> bool {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// 標準入力に`input`を渡して成功したか。
pub(super) fn piped(cmd: &mut Command, input: &str) -> bool {
    piped_text(cmd, input).is_some()
}

/// 標準入力に`input`を渡し、成功したときの標準出力を返す。
pub(super) fn piped_text(cmd: &mut Command, input: &str) -> Option<String> {
    use std::io::Write;
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let _ = child
        .stdin
        .take()
        .map(|mut i| i.write_all(input.as_bytes()));
    let out = child.wait_with_output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// jqに`input`を渡して`-e`の結果が真か。
pub(super) fn jq_input(args: &[&str], input: &str) -> bool {
    piped(Command::new("jq").args(args), input)
}

/// `limit`を過ぎたら止める。成功したときの標準出力。出力を読みながら待つので、大きなAXの木でも詰まらない。
pub(super) fn limited(cmd: &mut Command, limit: Duration) -> Option<String> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let rx = read_all(child.stdout.take());
    let out = rx.recv_timeout(limit).ok();
    let _ = child.kill();
    let success = child.wait().is_ok_and(|s| s.success());
    out.filter(|_| success)
}

pub(super) fn read_all(pipe: Option<ChildStdout>) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = pipe.map(|mut p| p.read_to_string(&mut s));
        let _ = tx.send(s);
    });
    rx
}

/// 上限つきで終わりを待ち、過ぎたら止めて回収する。
pub(super) fn reap(mut child: Child, limit: Duration) {
    crate::wait::wait_until(limit, STEP, || child.try_wait().is_ok_and(|s| s.is_some()));
    let _ = child.kill();
    let _ = child.wait();
}

pub(super) fn first(lines: &str) -> String {
    lines.lines().next().unwrap_or_default().trim().to_string()
}

pub(super) fn show_pid(pid: Option<u32>) -> String {
    pid.map(|p| p.to_string()).unwrap_or_default()
}

/// colimaのdefaultが動いているか。
pub(super) fn colima_running() -> bool {
    text(Command::new("colima").args(["list", "--json"])).is_some_and(|list| {
        jq_input(
            &[
                "-es",
                r#"any(.[]; .name == "default" and .status == "Running")"#,
            ],
            &list,
        )
    })
}

/// Dockerのソケットはcolimaに聞く（利用者のdocker contextにも、colimaの置き場所にも頼らない）。値はunix://付き。
pub(super) fn socket_of(profile: &str) -> Option<String> {
    let status = text(Command::new("colima").args(["status", profile, "--json"]))?;
    let sock = piped_text(
        Command::new("jq").args(["-r", ".docker_socket // empty"]),
        &status,
    )?;
    Some(sock.trim().to_string()).filter(|s| !s.is_empty())
}

/// 何かがつながるか、自分で受けられなければ使用中。
pub(super) fn port_in_use(port: u16) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(500)).is_ok()
        || TcpListener::bind(addr).is_err()
}
