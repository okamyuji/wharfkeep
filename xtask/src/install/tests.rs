use super::*;
use std::os::unix::fs::PermissionsExt;
use std::process::Child;

const PRINT_RUNNING: &str = "gui/501/wharfkeep = {\n\tactive count = 4\n\tpath = /Users/u/Library/LaunchAgents/wharfkeep.plist\n\ttype = LaunchAgent\n\tstate = running\n\n\tprogram = /Applications/wharfkeep.app/Contents/MacOS/wharfkeep\n\tendpoints = {\n\t\tstate = active\n\t}\n}\n";

fn launch_agent(exe: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>wharfkeep</string>\n  <key>ProgramArguments</key><array><string>{exe}</string><string>daemon</string></array>\n</dict>\n</plist>\n"
    )
}

#[test]
fn parse_args_picks_the_source() {
    assert_eq!(parse_args(&[]), Ok(Source::Release));
    assert_eq!(parse_args(&["--local".to_string()]), Ok(Source::Local));
    assert!(parse_args(&["--bogus".to_string()]).is_err());
    assert!(parse_args(&["--local".to_string(), "x".to_string()]).is_err());
}

#[test]
fn no_args_refuses_any_argument() {
    assert_eq!(no_args(&[]), Ok(()));
    assert!(no_args(&["x".to_string()]).is_err());
}

#[test]
fn holder_accepts_only_a_positive_pid() {
    assert_eq!(holder("123\n"), Some(123));
    assert_eq!(holder("1"), Some(1));
    for bad in ["0", "-1", "", "abc", "12 3"] {
        assert_eq!(holder(bad), None, "{bad}");
    }
}

#[test]
fn scratch_is_removed_when_dropped() {
    let s = Scratch::new().unwrap();
    let dir = s.0.clone();
    fs::write(dir.join("f"), "").unwrap();
    drop(s);
    assert!(!dir.exists());
}

#[test]
fn setup_places_the_app_the_login_item_and_the_data() {
    let s = Setup::new(
        Path::new("/A"),
        Path::new("/Users/u"),
        "501",
        Duration::ZERO,
    );
    assert_eq!(s.app(), Path::new("/A/wharfkeep.app"));
    assert_eq!(
        s.exe(),
        Path::new("/A/wharfkeep.app/Contents/MacOS/wharfkeep")
    );
    assert_eq!(
        s.plist(),
        Path::new("/Users/u/Library/LaunchAgents/wharfkeep.plist")
    );
    assert_eq!(
        s.data(),
        Path::new("/Users/u/Library/Application Support/work.okamyuji.wharfkeep")
    );
    assert_eq!(s.service, "gui/501/wharfkeep");
    assert_eq!(s.launchctl, Path::new("/bin/launchctl"));
    assert_eq!(s.open, Path::new("/usr/bin/open"));
    assert_eq!(s.gh, Path::new("gh"));
}

#[test]
fn string_after_reads_the_value_that_follows_the_key() {
    let info = include_str!("../../../assets/Info.plist");
    assert_eq!(
        string_after(info, "CFBundleShortVersionString"),
        Some("0.1.0")
    );
    let agent = launch_agent("/Applications/wharfkeep.app/Contents/MacOS/wharfkeep");
    assert_eq!(
        string_after(&agent, "ProgramArguments"),
        Some("/Applications/wharfkeep.app/Contents/MacOS/wharfkeep")
    );
    assert_eq!(string_after(&agent, "Missing"), None);
    assert_eq!(string_after("<key>K</key><string>open", "K"), None);
    assert_eq!(string_after("<key>K</key>", "K"), None);
}

#[test]
fn is_running_reads_only_the_job_state() {
    assert!(is_running(PRINT_RUNNING));
    assert!(!is_running(
        &PRINT_RUNNING.replace("state = running", "state = not running")
    ));
    assert!(!is_running(""));
}

#[test]
fn verdict_names_what_is_wrong() {
    let exe = Path::new("/Applications/wharfkeep.app/Contents/MacOS/wharfkeep");
    let good = launch_agent(&exe.display().to_string());
    assert_eq!(verdict(PRINT_RUNNING, Some(&good), exe), Ok(()));
    let off = verdict(PRINT_RUNNING, None, exe).unwrap_err();
    assert!(off.contains("plist がありません"), "{off}");
    let other = launch_agent("/repo/target/wharfkeep.app/Contents/MacOS/wharfkeep");
    let wrong = verdict(PRINT_RUNNING, Some(&other), exe).unwrap_err();
    assert!(wrong.contains("/repo/target/wharfkeep.app"), "{wrong}");
    let stopped = verdict("state = not running", Some(&good), exe).unwrap_err();
    assert!(stopped.contains("running"), "{stopped}");
}

#[test]
fn single_zip_takes_exactly_one_zip() {
    let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(
        single_zip(&names(&["wharfkeep-0.1.1-macos-arm64.zip", "notes.txt"])),
        Ok("wharfkeep-0.1.1-macos-arm64.zip".to_string())
    );
    assert!(single_zip(&names(&[])).is_err());
    assert!(single_zip(&names(&["a.zip", "b.zip"])).is_err());
}

/// 偽のlaunchctlとopen。launchdの状態は`state/`のファイルで表す。
struct Fake {
    _tmp: tempfile::TempDir,
    s: Setup,
    log: PathBuf,
    built: PathBuf,
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// 開いたアプリが書くplist。
enum Writes {
    /// /Applicationsの実行ファイルを指す。
    Own,
    /// 別の実行ファイルを指す。
    Other(&'static str),
    /// 書かない（自動起動がオフ）。
    Nothing,
}

/// `starts`が偽なら、kickstartしてもlaunchdは動かない。
fn fake(writes: Writes, starts: bool) -> Fake {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let apps = root.join("Applications");
    let home = root.join("home");
    let state = root.join("state");
    for d in [&apps, &home, &state] {
        fs::create_dir_all(d).unwrap();
    }
    let log = root.join("log");
    let mut s = Setup::new(&apps, &home, "501", Duration::from_millis(1500));
    s.launchctl = root.join("launchctl");
    s.open = root.join("open");
    s.gh = root.join("gh");
    let st = state.display();
    let kick = if starts {
        format!("[ -e {st}/loaded ] && touch {st}/running")
    } else {
        ":".into()
    };
    script(
        &s.launchctl,
        &format!(
            "echo \"launchctl $*\" >> {log}\ncase \"$1\" in\n  print) [ -e {st}/loaded ] || exit 113; if [ -e {st}/running ]; then echo 'state = running'; else echo 'state = not running'; fi ;;\n  bootout) rm -f {st}/loaded {st}/running ;;\n  kickstart) {kick} ;;\nesac\n",
            log = log.display()
        ),
    );
    let exe = match writes {
        Writes::Own => Some(s.exe().display().to_string()),
        Writes::Other(p) => Some(p.to_string()),
        Writes::Nothing => None,
    };
    let register = match exe {
        Some(exe) => {
            let agent = root.join("agent.plist");
            fs::write(&agent, launch_agent(&exe)).unwrap();
            format!(
                "mkdir -p {dir}; cp {agent} {plist}; touch {st}/loaded\n",
                dir = s.plist().parent().unwrap().display(),
                agent = agent.display(),
                plist = s.plist().display()
            )
        }
        None => String::new(),
    };
    script(
        &s.open,
        &format!("echo \"open $*\" >> {}\n{register}", log.display()),
    );
    let built = root.join("built/wharfkeep.app");
    fs::create_dir_all(built.join("Contents/MacOS")).unwrap();
    fs::write(
        built.join("Contents/Info.plist"),
        include_str!("../../../assets/Info.plist").replace("0.1.0", "0.1.7"),
    )
    .unwrap();
    fs::write(built.join("Contents/MacOS/wharfkeep"), "new").unwrap();
    Fake {
        _tmp: tmp,
        s,
        log,
        built,
    }
}

impl Fake {
    fn log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

#[test]
fn install_replaces_the_app_and_hands_the_daemon_to_launchd() {
    let f = fake(Writes::Own, true);
    // 前の版の.appを置く。中の古いファイルは置き換えで消える。
    fs::create_dir_all(f.s.app().join("Contents")).unwrap();
    fs::write(f.s.app().join("Contents/old"), "").unwrap();

    assert_eq!(install(&f.s, &f.built), Ok("0.1.7".to_string()));
    assert_eq!(fs::read(f.s.exe()).unwrap(), b"new");
    assert!(!f.s.app().join("Contents/old").exists());
    let left: Vec<_> = fs::read_dir(&f.s.apps).unwrap().collect();
    assert_eq!(left.len(), 1, "一時の名前が残っています: {left:?}");
    let log = f.log();
    let order: Vec<&str> = log.lines().collect();
    assert_eq!(order[0], format!("launchctl bootout {}", f.s.service));
    assert_eq!(order[1], format!("open {}", f.s.app().display()));
    assert!(
        log.contains("launchctl kickstart gui/501/wharfkeep"),
        "{log}"
    );
}

#[test]
fn install_waits_for_the_opened_app_to_register_again() {
    let f = fake(Writes::Own, true);
    // 前の導入のplistが、同じ/Applicationsを指したまま残っている。
    fs::create_dir_all(f.s.plist().parent().unwrap()).unwrap();
    fs::write(f.s.plist(), launch_agent(&f.s.exe().display().to_string())).unwrap();
    // 開いた.appは、少し遅れてplistを書き直し、launchdに登録し直す。
    let open = fs::read_to_string(&f.s.open)
        .unwrap()
        .replace("mkdir -p", "(sleep 0.5; mkdir -p")
        .replace("/loaded\n", "/loaded) &\n");
    script(&f.s.open, open.trim_start_matches("#!/bin/sh\n"));
    assert_eq!(install(&f.s, &f.built), Ok("0.1.7".to_string()));
}

#[test]
fn install_reports_a_login_item_that_was_not_written() {
    let f = fake(Writes::Nothing, true);
    let e = install(&f.s, &f.built).unwrap_err();
    assert!(e.contains("plist がありません"), "{e}");
    // 開いたデーモンを止めて渡す手順には進まない。
    assert!(!f.log().contains("kickstart"), "{}", f.log());
}

#[test]
fn install_reports_a_login_item_for_another_copy() {
    let f = fake(
        Writes::Other("/repo/target/wharfkeep.app/Contents/MacOS/wharfkeep"),
        true,
    );
    let e = install(&f.s, &f.built).unwrap_err();
    assert!(e.contains("/repo/target/wharfkeep.app"), "{e}");
    assert!(!f.log().contains("kickstart"), "{}", f.log());
}

#[test]
fn install_reports_a_daemon_that_launchd_does_not_run() {
    let f = fake(Writes::Own, false);
    let e = install(&f.s, &f.built).unwrap_err();
    assert!(e.contains("running"), "{e}");
}

#[test]
fn install_stops_before_touching_the_daemon_when_applications_is_read_only() {
    let f = fake(Writes::Own, true);
    fs::set_permissions(&f.s.apps, fs::Permissions::from_mode(0o555)).unwrap();
    let e = install(&f.s, &f.built);
    fs::set_permissions(&f.s.apps, fs::Permissions::from_mode(0o755)).unwrap();
    let e = e.unwrap_err();
    assert!(e.contains("書けません"), "{e}");
    assert!(e.contains(&f.s.apps.display().to_string()), "{e}");
    assert_eq!(f.log(), "");
    assert!(!f.s.app().exists());
}

/// 偽のghに、組んだ.appのzipを--dirの先へ置かせる。
fn serve_release(f: &Fake) {
    let zips = f.s.apps.parent().unwrap().join("zips");
    fs::create_dir(&zips).unwrap();
    let zip = zips.join("wharfkeep-0.1.7-macos-arm64.zip");
    check(
        Command::new("ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(&f.built)
            .arg(&zip),
    )
    .unwrap();
    // 偽のghは --dir の先へzipを置き、渡された引数を記録する。
    script(
        &f.s.gh,
        &format!(
            "echo \"gh $*\" >> {log}\nwhile [ \"$1\" != --dir ]; do shift; done\ncp {zip} \"$2\"/\n",
            log = f.log.display(),
            zip = zip.display()
        ),
    );
}

#[test]
fn download_unpacks_the_single_release_zip() {
    let f = fake(Writes::Nothing, true);
    serve_release(&f);
    let dir = tempfile::tempdir().unwrap();
    let app = download(&f.s, dir.path()).unwrap();
    assert_eq!(
        fs::read(app.join("Contents/MacOS/wharfkeep")).unwrap(),
        b"new"
    );
    assert!(
        f.log()
            .contains("gh release download --repo okamyuji/wharfkeep --pattern *.zip --dir"),
        "{}",
        f.log()
    );
}

#[test]
fn install_with_takes_the_release_by_default() {
    let f = fake(Writes::Own, true);
    serve_release(&f);
    install_with(&f.s, &[], || panic!("must not build")).unwrap();
    assert_eq!(fs::read(f.s.exe()).unwrap(), b"new");
    assert!(f.log().contains("gh release download"), "{}", f.log());
}

#[test]
fn install_with_builds_and_takes_the_local_app_with_local() {
    let f = fake(Writes::Own, true);
    let built = f.built.clone();
    install_with(&f.s, &["--local".to_string()], || Ok(built)).unwrap();
    assert_eq!(fs::read(f.s.exe()).unwrap(), b"new");
    assert!(!f.log().contains("gh "), "{}", f.log());
    let e = install_with(&f.s, &["--bogus".to_string()], || panic!("must not build"));
    assert!(e.unwrap_err().contains("--local"));
    let e = install_with(
        &f.s,
        &["--local".to_string()],
        || Err("build failed".into()),
    );
    assert_eq!(e, Err("build failed".to_string()));
}

#[test]
fn uninstall_removes_the_app_and_the_login_item_but_keeps_the_data() {
    let f = fake(Writes::Nothing, true);
    fs::create_dir_all(f.s.app().join("Contents")).unwrap();
    fs::create_dir_all(f.s.plist().parent().unwrap()).unwrap();
    fs::write(f.s.plist(), "x").unwrap();
    fs::create_dir_all(f.s.data()).unwrap();
    fs::write(f.s.data().join("state.json"), "{}").unwrap();

    uninstall(&f.s).unwrap();
    assert!(!f.s.app().exists());
    assert!(!f.s.plist().exists());
    assert!(f.s.data().join("state.json").exists());
    assert!(f.log().starts_with("launchctl bootout gui/501/wharfkeep"));
    // 2度目は消すものがなくても成功する。
    uninstall(&f.s).unwrap();
    // 消せないplistは知らせる。
    fs::create_dir(f.s.plist()).unwrap();
    let e = uninstall(&f.s).unwrap_err();
    assert!(e.contains("wharfkeep.plist"), "{e}");
}

/// ロックを持ったまま眠るperl。`InstanceLock`と同じくflockで取り、pidを書く。
fn hold_lock(lock: &Path) -> Child {
    let child = Command::new("/usr/bin/perl")
        .args([
            "-e",
            "use Fcntl qw(:flock); open(my $f, '>>', $ARGV[0]) or die; flock($f, LOCK_EX) or die; truncate($f, 0); syswrite($f, $$); sleep 30;",
        ])
        .arg(lock)
        .spawn()
        .unwrap();
    let pid = child.id().to_string();
    // wait_untilは試す対象なので使わない。
    let end = std::time::Instant::now() + Duration::from_secs(10);
    while !(held(lock) && fs::read_to_string(lock).is_ok_and(|s| s == pid)) {
        assert!(
            std::time::Instant::now() < end,
            "perl did not take the lock"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    child
}

#[test]
fn stop_holder_ends_the_process_that_holds_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join("daemon.lock");
    let mut child = hold_lock(&lock);
    stop_holder(&lock, Duration::from_secs(10)).unwrap();
    assert!(!held(&lock));
    assert!(!child.wait().unwrap().success());
    // ロックが無い、持ち主がいないときは何もしない。
    stop_holder(&dir.path().join("none.lock"), Duration::ZERO).unwrap();
    stop_holder(&lock, Duration::ZERO).unwrap();
}

#[test]
fn stop_holder_reports_a_holder_that_does_not_end() {
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join("daemon.lock");
    let mut child = hold_lock(&lock);
    // 持ち主と違うpidが書かれていると、送った信号は届かない。
    fs::write(&lock, "999999").unwrap();
    let e = stop_holder(&lock, Duration::from_millis(300)).unwrap_err();
    assert!(e.contains("止まりません"), "{e}");
    child.kill().unwrap();
    child.wait().unwrap();
}
