use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const STALE_AFTER: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, PartialEq, Eq)]
pub enum TerminalError {
    EmptyArgv,
    InvalidEnvKey(String),
}

impl fmt::Display for TerminalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyArgv => write!(f, "command is empty"),
            Self::InvalidEnvKey(k) => write!(f, "invalid environment variable name: {k:?}"),
        }
    }
}

impl std::error::Error for TerminalError {}

/// シングルクォートで囲む。`'`は`'\''`に置き換える。
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn is_valid_env_key(k: &str) -> bool {
    let mut chars = k.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// `export`行と`exec`行を持つシェルスクリプトを作る。
pub fn command_script(argv: &[String], env: &[(String, String)]) -> Result<String, TerminalError> {
    if argv.is_empty() {
        return Err(TerminalError::EmptyArgv);
    }
    // 環境変数の値（トークンなど）を含むので、実行され始めた時点で自分を消す。
    let mut out = String::from("#!/bin/sh\nrm -f -- \"$0\"\n");
    for (k, v) in env {
        if !is_valid_env_key(k) {
            return Err(TerminalError::InvalidEnvKey(k.clone()));
        }
        out.push_str(&format!("export {k}={}\n", shell_quote(v)));
    }
    let args: Vec<String> = argv.iter().map(|a| shell_quote(a)).collect();
    out.push_str(&format!("exec -- {}\n", args.join(" ")));
    Ok(out)
}

/// 24時間より古い`.command`を消す。1件の失敗でターミナルを開けなくしないので、エラーは無視する。
fn remove_stale(dir: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "command") {
            continue;
        }
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok());
        if age.is_some_and(|a| a > STALE_AFTER) {
            let _ = fs::remove_file(&path);
        }
    }
}

fn unique_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{}-{nanos}-{n}.command", std::process::id())
}

/// 引数の検証が先なので、不正な入力ではファイルを作らない。
fn write_script_in(dir: &Path, argv: &[String], env: &[(String, String)]) -> io::Result<PathBuf> {
    let script =
        command_script(argv, env).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    // mode は新規作成のときだけ効く。以前に緩い権限で作られた dir も締める。
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    remove_stale(dir, SystemTime::now());
    let path = dir.join(unique_name());
    // 0600で作って書き終えてから0700にする。他ユーザーが実行可能な状態を作らない。
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    file.write_all(script.as_bytes())?;
    drop(file);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    Ok(path)
}

fn terminal_dir(data_dir: Option<PathBuf>) -> io::Result<PathBuf> {
    data_dir
        .map(|d| d.join("terminal"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "home directory is unknown"))
}

/// opener は試験で差し替えるために引数にしている。
fn open_with(
    opener: &Path,
    dir: &Path,
    argv: &[String],
    env: &[(String, String)],
) -> io::Result<()> {
    let path = write_script_in(dir, argv, env)?;
    let status = Command::new(opener).arg(&path).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("open exited with {status}")))
    }
}

/// `.command`を書いて`open`する。`.command`に関連付けられたアプリ（既定はTerminal.app）が実行する。
pub fn open_in_terminal(argv: &[String], env: &[(String, String)]) -> io::Result<()> {
    let dir = terminal_dir(crate::paths::data_dir())?;
    open_with(Path::new("/usr/bin/open"), &dir, argv, env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::time::Duration;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn run_sh(script: &str) -> String {
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    #[test]
    fn quotes_space_quote_and_dollar() {
        let script = command_script(&s(&["printf", "%s", "a b'c$HOME"]), &[]).unwrap();
        assert_eq!(run_sh(&script), "a b'c$HOME");
    }

    #[test]
    fn exports_env_before_command() {
        let env = vec![("DOCKER_HOST".to_string(), "unix:///x y".to_string())];
        let script = command_script(&s(&["sh", "-c", "printf %s \"$DOCKER_HOST\""]), &env).unwrap();
        assert_eq!(run_sh(&script), "unix:///x y");
    }

    #[test]
    fn script_starts_with_shebang() {
        let script = command_script(&s(&["true"]), &[]).unwrap();
        assert!(script.starts_with("#!/bin/sh\n"));
    }

    #[test]
    fn empty_argv_is_rejected() {
        assert_eq!(command_script(&[], &[]), Err(TerminalError::EmptyArgv));
    }

    #[test]
    fn invalid_env_key_is_rejected() {
        for key in ["", "1A", "A-B", "A B", "A=B", "A\n"] {
            let env = vec![(key.to_string(), "v".to_string())];
            assert_eq!(
                command_script(&s(&["true"]), &env),
                Err(TerminalError::InvalidEnvKey(key.to_string())),
                "{key:?}"
            );
        }
    }

    #[test]
    fn valid_env_keys_are_accepted() {
        for key in ["A", "_", "a1_B"] {
            let env = vec![(key.to_string(), "v".to_string())];
            assert!(command_script(&s(&["true"]), &env).is_ok(), "{key}");
        }
    }

    #[test]
    fn error_messages_name_the_cause() {
        assert!(TerminalError::EmptyArgv.to_string().contains("empty"));
        assert!(
            TerminalError::InvalidEnvKey("1A".into())
                .to_string()
                .contains("1A")
        );
    }

    #[test]
    fn script_file_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_script_in(dir.path(), &s(&["true"]), &[]).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        assert_eq!(path.extension().unwrap(), "command");
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("exec -- 'true'")
        );
    }

    #[test]
    fn written_scripts_have_unique_names() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_script_in(dir.path(), &s(&["true"]), &[]).unwrap();
        let b = write_script_in(dir.path(), &s(&["true"]), &[]).unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn invalid_input_writes_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = write_script_in(dir.path(), &[], &[]).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn creates_missing_dir_with_owner_only_mode() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("terminal");
        write_script_in(&sub, &s(&["true"]), &[]).unwrap();
        assert_eq!(
            fs::metadata(&sub).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn argv0_starting_with_dash_is_executed_not_parsed_as_exec_option() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("-x");
        fs::write(&bin, "#!/bin/sh\nprintf dash-ok\n").unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let script = command_script(&s(&["-x"]), &[]).unwrap();
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .env("PATH", dir.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        assert_eq!(out.stdout, b"dash-ok");
    }

    #[test]
    fn script_removes_itself_before_running_the_command() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_script_in(dir.path(), &s(&["true"]), &[]).unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert_eq!(body.lines().nth(1), Some("rm -f -- \"$0\""));
        let st = Command::new("/bin/sh").arg(&path).status().unwrap();
        assert!(st.success());
        assert!(!path.exists());
    }

    #[test]
    fn script_removes_itself_even_when_the_command_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_script_in(dir.path(), &s(&["false"]), &[]).unwrap();
        let st = Command::new("/bin/sh").arg(&path).status().unwrap();
        assert_eq!(st.code(), Some(1));
        assert!(!path.exists());
    }

    #[test]
    fn existing_loose_dir_is_tightened_to_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("terminal");
        fs::create_dir(&sub).unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
        write_script_in(&sub, &s(&["true"]), &[]).unwrap();
        assert_eq!(
            fs::metadata(&sub).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    fn touch(dir: &Path, name: &str, now: SystemTime, age: Duration) -> PathBuf {
        let p = dir.join(name);
        let f = File::create(&p).unwrap();
        f.set_modified(now - age).unwrap();
        p
    }

    #[test]
    fn removes_only_command_files_older_than_24h() {
        let dir = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let h = |n: u64| Duration::from_secs(n * 3600);
        let old = touch(dir.path(), "old.command", now, h(25));
        let edge = touch(dir.path(), "edge.command", now, h(24));
        let fresh = touch(dir.path(), "fresh.command", now, h(1));
        let other = touch(dir.path(), "old.txt", now, h(48));
        remove_stale(dir.path(), now);
        assert!(!old.exists());
        assert!(edge.exists());
        assert!(fresh.exists());
        assert!(other.exists());
    }

    #[test]
    fn stale_removal_failure_does_not_block_writing() {
        let dir = tempfile::tempdir().unwrap();
        // 古い.commandという名前のディレクトリはremove_fileで消せない。
        let stuck = dir.path().join("stuck.command");
        fs::create_dir(&stuck).unwrap();
        File::open(&stuck)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(48 * 3600))
            .unwrap();
        assert!(write_script_in(dir.path(), &s(&["true"]), &[]).is_ok());
    }

    fn fake_opener(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        p
    }

    #[test]
    fn open_with_passes_generated_command_path_to_opener() {
        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join("arg.txt");
        let opener = fake_opener(
            dir.path(),
            "ok.sh",
            &format!("printf '%s' \"$1\" > '{}'", record.display()),
        );
        let target = dir.path().join("terminal");
        open_with(&opener, &target, &s(&["true"]), &[]).unwrap();
        let arg = PathBuf::from(fs::read_to_string(&record).unwrap());
        assert_eq!(arg.parent().unwrap(), target);
        assert_eq!(arg.extension().unwrap(), "command");
        assert!(arg.exists());
    }

    #[test]
    fn open_with_fails_when_opener_exits_nonzero() {
        let dir = tempfile::tempdir().unwrap();
        let opener = fake_opener(dir.path(), "ng.sh", "exit 1");
        let err = open_with(&opener, &dir.path().join("t"), &s(&["true"]), &[]).unwrap_err();
        assert!(err.to_string().contains("exited"), "{err}");
    }

    #[test]
    fn open_with_fails_when_opener_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("none");
        assert!(open_with(&missing, &dir.path().join("t"), &s(&["true"]), &[]).is_err());
    }

    #[test]
    fn terminal_dir_maps_missing_data_dir_to_not_found() {
        let err = terminal_dir(None).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert_eq!(
            terminal_dir(Some(PathBuf::from("/x"))).unwrap(),
            PathBuf::from("/x/terminal")
        );
    }

    #[test]
    #[ignore = "実際に既定のターミナルを開く"]
    fn opens_real_terminal() {
        let marker = format!("wharfkeep-term-test-{}", std::process::id());
        open_in_terminal(&s(&["sh", "-c", "sleep 30; true", &marker]), &[]).unwrap();
        std::thread::sleep(Duration::from_secs(5));
        let out = Command::new("pgrep")
            .args(["-fl", &marker])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(text.contains(&marker), "pgrep: {text:?}");
        Command::new("pkill")
            .args(["-f", &marker])
            .status()
            .unwrap();
    }
}
