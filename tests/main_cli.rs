//! デーモンやGUIを立てずに通れる、mainの分かれ道を子プロセスで確かめる。
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};
use wharfkeep::lock::InstanceLock;

/// ロックを取り損ねるとデーモンが立ち上がって戻らないので、時間を区切って止める。
fn run_with_home(home: &Path, arg: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_wharfkeep"))
        .arg(arg)
        .env("HOME", home)
        .env("WHARFKEEP_LANG", "en")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run wharfkeep");
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().expect("poll wharfkeep").is_none() {
        if Instant::now() > deadline {
            child.kill().expect("kill wharfkeep");
            panic!("wharfkeep {arg} did not exit");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().expect("collect wharfkeep output")
}

#[test]
fn a_second_daemon_exits_quietly_while_the_lock_is_held() {
    let home = tempfile::tempdir().unwrap();
    let dir = home
        .path()
        .join("Library/Application Support/work.okamyuji.wharfkeep");
    std::fs::create_dir_all(&dir).unwrap();
    let held = InstanceLock::try_acquire(&dir.join("daemon.lock")).unwrap();
    assert!(held.is_some());
    let out = run_with_home(home.path(), "daemon");
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn an_unusable_data_dir_is_reported() {
    let home = tempfile::tempdir().unwrap();
    std::fs::write(home.path().join("Library"), "").unwrap();
    let out = run_with_home(home.path(), "daemon");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cannot take the lock"));
}
