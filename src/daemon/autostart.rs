//! ログイン時の自動起動。`~/Library/LaunchAgents/wharfkeep.plist`に`wharfkeep daemon`を登録する。

use serde::{Deserialize, Serialize};
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const APP_NAME: &str = "wharfkeep";

/// `data_dir()/settings.json`の中身。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub autostart: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { autostart: true }
    }
}

/// ファイルがないか読めなければ既定値（自動起動は有効）を返す。
pub fn load_settings(path: &Path) -> Settings {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_settings(path: &Path, s: &Settings) -> io::Result<()> {
    std::fs::write(path, serde_json::to_vec(s).map_err(io::Error::other)?)
}

/// launchdのLaunchAgent。auto-launchは依存にMPL-2.0（option-ext）を含むので使わず、plistを自分で書く。
pub struct Autostart {
    exe: PathBuf,
    plist: PathBuf,
}

impl Autostart {
    /// `exe`は絶対パスでなければならない。launchdは作業ディレクトリを決めずに起動する。
    pub fn new(exe: &Path) -> Result<Autostart, String> {
        if !exe.is_absolute() {
            return Err(format!("not an absolute path: {}", exe.display()));
        }
        let home = std::env::home_dir().ok_or("no home directory")?;
        Ok(Autostart {
            exe: exe.to_path_buf(),
            plist: home.join(format!("Library/LaunchAgents/{APP_NAME}.plist")),
        })
    }

    /// plistを書くだけで、launchdには読み込ませない。
    pub fn enable(&self) -> Result<(), String> {
        if !self.exe.is_file() {
            return Err(format!("no such file: {}", self.exe.display()));
        }
        let io_err = |e: io::Error| format!("{}: {e}", self.plist.display());
        if let Some(dir) = self.plist.parent() {
            std::fs::create_dir_all(dir).map_err(io_err)?;
        }
        std::fs::write(&self.plist, plist_xml(&self.exe)).map_err(io_err)
    }

    pub fn disable(&self) -> Result<(), String> {
        match std::fs::remove_file(&self.plist) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                Err(format!("{}: {e}", self.plist.display()))
            }
            _ => Ok(()),
        }
    }

    pub fn is_enabled(&self) -> Result<bool, String> {
        self.plist.try_exists().map_err(|e| e.to_string())
    }

    /// `~/Library/LaunchAgents/wharfkeep.plist`。
    pub fn plist_path(&self) -> &Path {
        &self.plist
    }
}

/// 異常終了のときだけ起動し直す（`KeepAlive`の`SuccessfulExit=false`）。メニューの「終了」は0で終わるので、
/// 利用者が止めたデーモンは起こさない。
fn plist_xml(exe: &Path) -> String {
    let exe = xml_escape(&exe.to_string_lossy());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{APP_NAME}</string>
  <key>ProgramArguments</key><array><string>{exe}</string><string>daemon</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
</dict>
</plist>
"#
    )
}

fn xml_escape(s: &str) -> String {
    s.chars()
        .fold(String::with_capacity(s.len()), |mut out, c| {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&apos;"),
                _ => out.push(c),
            }
            out
        })
}

const LAUNCHCTL: &str = "/bin/launchctl";

/// `on`なら`exe`でplistを書き直し、launchdに読み込まれていなければ読み込ませる。`on`でなければplistを消し、
/// デーモンが終わったあとでlaunchdから外す。
pub fn apply(exe: &Path, on: bool) -> Result<(), String> {
    apply_with(Path::new(LAUNCHCTL), exe, on)
}

fn apply_with(launchctl: &Path, exe: &Path, on: bool) -> Result<(), String> {
    let a = Autostart::new(exe)?;
    if !on {
        a.disable()?;
        return unload_when_daemon_exits(launchctl, std::process::id(), a.plist_path());
    }
    a.enable()?;
    load_into_launchd(launchctl, a.plist_path())
}

fn service_target() -> String {
    // SAFETY: getuidは失敗せず、メモリにも触れない。
    format!("gui/{}/{APP_NAME}", unsafe { libc::getuid() })
}

/// 読み込み済みのジョブは、plistを消してもlaunchdに残る。残ると`kill -9`などで落ちたデーモンを起こし直す。
/// 今のデーモンを止めずに外すため、`pid`の終わりを待って`bootout`する別プロセスを置く。
/// 別のプロセスグループに置くのは、launchdがジョブの終了時にグループごと片付けるため。
/// 待つ間に自動起動が有効へ戻された（plistがある）なら外さない。
fn unload_when_daemon_exits(launchctl: &Path, pid: u32, plist: &Path) -> Result<(), String> {
    const WATCH: &str = r#"while kill -0 "$1" 2>/dev/null; do sleep 1; done; [ -e "$2" ] || exec "$4" bootout "$3""#;
    let mut child = Command::new("/bin/sh")
        .args(["-c", WATCH, "wharfkeep-unload"])
        .arg(pid.to_string())
        .arg(plist)
        .arg(service_target())
        .arg(launchctl)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| e.to_string())?;
    // 終わった子がデーモンの終了までゾンビで残らないよう回収する。
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// plistを書いただけでは次のログインまで読み込まれない。今すぐ登録済みにする。
/// 読み込むとlaunchdがもう1つデーモンを起こすが、ロックを取れずに0で終わり、起こし直されない。
fn load_into_launchd(launchctl: &Path, plist: &Path) -> Result<(), String> {
    // SAFETY: getuidは失敗せず、メモリにも触れない。
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let launchctl = |args: &[&std::ffi::OsStr]| {
        Command::new(launchctl)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| e.to_string())
    };
    let service = format!("{domain}/{APP_NAME}");
    if launchctl(&["print".as_ref(), service.as_ref()])?
        .status
        .success()
    {
        return Ok(());
    }
    let out = launchctl(&["bootstrap".as_ref(), domain.as_ref(), plist.as_os_str()])?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "launchctl bootstrap: {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_is_autostart_on() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load_settings(&dir.path().join("settings.json")),
            Settings { autostart: true }
        );
        assert_eq!(Settings::default(), Settings { autostart: true });
    }

    #[test]
    fn settings_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        save_settings(&path, &Settings { autostart: false }).unwrap();
        assert_eq!(load_settings(&path), Settings { autostart: false });
        save_settings(&path, &Settings { autostart: true }).unwrap();
        assert_eq!(load_settings(&path), Settings { autostart: true });
    }

    #[test]
    fn broken_settings_fall_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(load_settings(&path), Settings { autostart: true });
        std::fs::write(&path, r#"{"autostart":false,"later":1}"#).unwrap();
        assert_eq!(load_settings(&path), Settings { autostart: false });
    }

    #[test]
    fn save_into_missing_dir_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("none").join("settings.json");
        assert!(save_settings(&path, &Settings::default()).is_err());
    }

    #[test]
    fn relative_exe_is_rejected() {
        assert!(Autostart::new(Path::new("wharfkeep")).is_err());
    }

    #[test]
    fn missing_exe_is_rejected_before_writing() {
        let a = Autostart::new(Path::new("/nonexistent/wharfkeep")).unwrap();
        let err = a.enable().unwrap_err();
        assert!(err.contains("/nonexistent/wharfkeep"), "{err}");
    }

    #[test]
    fn disable_ignores_missing_plist_but_reports_other_errors() {
        let dir = tempfile::tempdir().unwrap();
        let missing = Autostart {
            exe: PathBuf::from("/usr/bin/true"),
            plist: dir.path().join("wharfkeep.plist"),
        };
        assert_eq!(missing.disable(), Ok(()));
        // ディレクトリはremove_fileで消せない。
        let dir_in_place = Autostart {
            exe: PathBuf::from("/usr/bin/true"),
            plist: dir.path().to_path_buf(),
        };
        assert!(dir_in_place.disable().is_err());
    }

    #[test]
    fn plist_escapes_xml_special_chars() {
        let xml = plist_xml(Path::new("/A&B/<x>/\"q'/w"));
        assert!(
            xml.contains(
                "<string>/A&amp;B/&lt;x&gt;/&quot;q&apos;/w</string><string>daemon</string>"
            ),
            "{xml}"
        );
        assert!(
            xml.contains("<key>Label</key><string>wharfkeep</string>"),
            "{xml}"
        );
        assert!(xml.contains("<key>RunAtLoad</key><true/>"), "{xml}");
    }

    /// 子プロセスの中で実際に登録と解除を行う。`HOME`を差し替えた子でだけ動き、利用者の登録には触れない。
    #[test]
    #[ignore = "plist_has_keepalive_and_daemon_arg から子プロセスとして呼ぶ"]
    fn child_enable_disable() {
        let Some(home) = std::env::var_os("WHARFKEEP_AUTOSTART_CHILD") else {
            return;
        };
        let home = PathBuf::from(home);
        // HOMEを差し替えた子でなければ、利用者の本物の登録を書き換えてしまう。
        assert_eq!(
            std::env::var_os("HOME"),
            Some(home.clone().into_os_string())
        );
        // XMLで特別な文字を含むパスでも、plistとして読めることを確かめる。
        let exe = home.join("a&b<c>");
        std::fs::write(&exe, "").unwrap();
        let a = Autostart::new(&exe).unwrap();
        assert!(!a.is_enabled().unwrap());
        a.enable().unwrap();
        assert!(a.is_enabled().unwrap());
        let path = home.join("Library/LaunchAgents/wharfkeep.plist");
        assert_eq!(a.plist_path(), path);
        let plist = std::fs::read_to_string(&path).unwrap();
        assert!(plist.contains("<string>daemon</string>"), "{plist}");
        let lint = std::process::Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(&path)
            .output()
            .unwrap();
        assert!(lint.status.success(), "{lint:?}");
        let args = std::process::Command::new("/usr/bin/plutil")
            .args(["-extract", "ProgramArguments.0", "raw"])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&args.stdout).trim(),
            exe.to_str().unwrap()
        );
        assert!(
            plist.contains("<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>"),
            "{plist}"
        );
        a.disable().unwrap();
        assert!(!a.is_enabled().unwrap());
    }

    /// 登録先はホームから決まる。環境変数の書き換えはプロセス全体に効き並列の試験と衝突するため、子プロセスで`HOME`を差し替える。
    fn run_child(test: &str) {
        let home = tempfile::tempdir().unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--ignored", "--nocapture"])
            .env("WHARFKEEP_AUTOSTART_CHILD", home.path())
            .env("HOME", home.path())
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && stdout.contains("1 passed"),
            "{stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn plist_has_keepalive_and_daemon_arg() {
        run_child("daemon::autostart::tests::child_enable_disable");
    }

    #[test]
    fn apply_toggles_plist_and_talks_to_launchctl() {
        run_child("daemon::autostart::tests::child_apply");
    }

    /// 引数を`log`に追記し、`print`だけ失敗する偽の launchctl。
    fn fake_launchctl(dir: &Path) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("launchctl");
        let log = dir.join("calls.log");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\necho \"$@\" >> '{}'\n[ \"$1\" = print ] && exit 1\nexit 0\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin, log)
    }

    fn calls(log: &Path) -> String {
        std::fs::read_to_string(log).unwrap_or_default()
    }

    /// 子プロセスの中で`apply_with`を呼ぶ。偽の launchctl を使うので、利用者の登録には触れない。
    #[test]
    #[ignore = "apply_toggles_plist_and_talks_to_launchctl から子プロセスとして呼ぶ"]
    fn child_apply() {
        let Some(home) = std::env::var_os("WHARFKEEP_AUTOSTART_CHILD") else {
            return;
        };
        let home = PathBuf::from(home);
        assert_eq!(
            std::env::var_os("HOME"),
            Some(home.clone().into_os_string())
        );
        let (bin, log) = fake_launchctl(&home);
        let exe = home.join("wharfkeep");
        std::fs::write(&exe, "").unwrap();
        let plist = home.join("Library/LaunchAgents/wharfkeep.plist");

        apply_with(&bin, &exe, true).unwrap();
        assert!(plist.exists());
        let c = calls(&log);
        assert!(c.contains("print gui/"), "{c}");
        assert!(c.contains("bootstrap gui/"), "{c}");
        assert!(c.contains(plist.to_str().unwrap()), "{c}");

        // 消すだけで、その場では launchctl を呼ばない。デーモンはこのまま動き続ける。
        let before = calls(&log);
        apply_with(&bin, &exe, false).unwrap();
        assert!(!plist.exists());
        assert_eq!(calls(&log), before);
    }

    fn uid() -> u32 {
        // SAFETY: getuidは失敗せず、メモリにも触れない。
        unsafe { libc::getuid() }
    }

    fn wait_for(log: &Path, needle: &str) -> bool {
        for _ in 0..60 {
            if calls(log).contains(needle) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    }

    #[test]
    fn launchd_job_is_removed_after_the_daemon_dies_without_a_plist() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, log) = fake_launchctl(dir.path());
        let mut daemon = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        unload_when_daemon_exits(&bin, daemon.id(), &dir.path().join("none.plist")).unwrap();
        // デーモンが生きている間は外さない。
        std::thread::sleep(std::time::Duration::from_millis(1500));
        assert_eq!(calls(&log), "");
        daemon.kill().unwrap();
        daemon.wait().unwrap();
        let want = format!("bootout gui/{}/wharfkeep", uid());
        assert!(wait_for(&log, &want), "{}", calls(&log));
    }

    #[test]
    fn launchd_job_stays_when_autostart_was_turned_back_on() {
        let dir = tempfile::tempdir().unwrap();
        let (bin, log) = fake_launchctl(dir.path());
        let plist = dir.path().join("wharfkeep.plist");
        std::fs::write(&plist, "").unwrap();
        let mut daemon = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        unload_when_daemon_exits(&bin, daemon.id(), &plist).unwrap();
        daemon.kill().unwrap();
        daemon.wait().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2500));
        assert_eq!(calls(&log), "");
    }
}
