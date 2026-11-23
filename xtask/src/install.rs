//! `/Applications/wharfkeep.app`の導入と取り外し。
use crate::bundle::{self, check, io_err};
use std::fs::{self, File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

const REPO: &str = "okamyuji/wharfkeep";
const APP: &str = "wharfkeep.app";

#[derive(Debug, PartialEq, Eq)]
enum Source {
    /// GitHubの最新のreleaseのzip。
    Release,
    /// `cargo xtask bundle --no-bump`で組んだtarget/wharfkeep.app。
    Local,
}

fn parse_args(args: &[String]) -> Result<Source, String> {
    match args {
        [] => Ok(Source::Release),
        [a] if a == "--local" => Ok(Source::Local),
        _ => Err(format!(
            "install の引数 {args:?} が分かりません（--local だけ）"
        )),
    }
}

/// 置き場と道具。試験は一時ディレクトリと偽の道具に差し替える。
pub(crate) struct Setup {
    apps: PathBuf,
    home: PathBuf,
    service: String,
    launchctl: PathBuf,
    open: PathBuf,
    gh: PathBuf,
    /// 1つの待ちの上限。
    wait: Duration,
}

impl Setup {
    fn new(apps: &Path, home: &Path, uid: &str, wait: Duration) -> Setup {
        Setup {
            apps: apps.to_path_buf(),
            home: home.to_path_buf(),
            service: format!("gui/{uid}/wharfkeep"),
            launchctl: "/bin/launchctl".into(),
            open: "/usr/bin/open".into(),
            gh: "gh".into(),
            wait,
        }
    }

    /// この利用者の/Applicationsとlaunchd。
    pub(crate) fn system() -> Result<Setup, String> {
        let home = std::env::home_dir().ok_or("ホームディレクトリが分かりません")?;
        let out = Command::new("/usr/bin/id")
            .arg("-u")
            .output()
            .map_err(|e| format!("id -u: {e}"))?;
        let uid = String::from_utf8_lossy(&out.stdout).trim().to_string();
        Ok(Setup::new(
            Path::new("/Applications"),
            &home,
            &uid,
            Duration::from_secs(20),
        ))
    }

    fn app(&self) -> PathBuf {
        self.apps.join(APP)
    }

    fn exe(&self) -> PathBuf {
        self.app().join("Contents/MacOS/wharfkeep")
    }

    /// デーモンがログイン時の起動のために書くLaunchAgent。
    fn plist(&self) -> PathBuf {
        self.home.join("Library/LaunchAgents/wharfkeep.plist")
    }

    fn data(&self) -> PathBuf {
        self.home
            .join("Library/Application Support/work.okamyuji.wharfkeep")
    }
}

pub fn run_install(root: &Path, args: &[String]) -> Result<(), String> {
    install_with(&Setup::system()?, args, || {
        bundle::run(root, &["--no-bump".to_string()]).map(|()| root.join("target").join(APP))
    })
}

/// `local`は`--local`のときに.appを組んで、その置き場を返す。
fn install_with(
    s: &Setup,
    args: &[String],
    local: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<(), String> {
    let source = parse_args(args)?;
    let tmp = Scratch::new().map_err(|e| format!("一時ディレクトリ: {e}"))?;
    let app = match source {
        Source::Local => local()?,
        Source::Release => download(s, &tmp.0)?,
    };
    let shown = install(s, &app)?;
    println!(
        "{} に版 {shown} を入れ、launchd で動かしました",
        s.app().display()
    );
    Ok(())
}

pub fn run_uninstall(args: &[String]) -> Result<(), String> {
    no_args(args)?;
    uninstall(&Setup::system()?)
}

pub(crate) fn no_args(args: &[String]) -> Result<(), String> {
    match args {
        [] => Ok(()),
        _ => Err(format!("この作業は引数を取りません: {args:?}")),
    }
}

/// 終わりに消す一時ディレクトリ。
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> io::Result<Scratch> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = format!("wharfkeep-install.{}.{n}", std::process::id());
        let dir = std::env::temp_dir().join(name);
        fs::create_dir(&dir)?;
        Ok(Scratch(dir))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 最新のreleaseのzipを`dir`に落として展開し、中の.appを返す。
fn download(s: &Setup, dir: &Path) -> Result<PathBuf, String> {
    check(
        Command::new(&s.gh)
            .args([
                "release",
                "download",
                "--repo",
                REPO,
                "--pattern",
                "*.zip",
                "--dir",
            ])
            .arg(dir),
    )?;
    let names = fs::read_dir(dir)
        .map_err(|e| io_err(dir, e))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect::<Vec<_>>();
    let zip = dir.join(single_zip(&names)?);
    let out = dir.join("unpacked");
    check(Command::new("ditto").args(["-x", "-k"]).arg(&zip).arg(&out))?;
    let app = out.join(APP);
    if !app.is_dir() {
        return Err(format!("{} に {APP} がありません", zip.display()));
    }
    Ok(app)
}

fn single_zip(names: &[String]) -> Result<String, String> {
    match names
        .iter()
        .filter(|n| n.ends_with(".zip"))
        .collect::<Vec<_>>()
        .as_slice()
    {
        [one] => Ok(one.to_string()),
        zips => Err(format!("release の zip が1つではありません: {zips:?}")),
    }
}

/// `app`を/Applicationsへ入れ、launchdで動かす。入れた版を返す。
fn install(s: &Setup, app: &Path) -> Result<String, String> {
    let info = app.join("Contents/Info.plist");
    let shown = string_after(
        &fs::read_to_string(&info).map_err(|e| io_err(&info, e))?,
        "CFBundleShortVersionString",
    )
    .ok_or_else(|| format!("{} に版がありません", info.display()))?
    .to_string();
    println!("入れる版: {shown}");
    // 書けるかを先に確かめる。書けなければデーモンに触らずに止まる。
    let staging = s.apps.join(format!(".{APP}.{}.new", std::process::id()));
    if let Err(e) = check(Command::new("ditto").arg(app).arg(&staging)) {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("{} に書けません: {e}", s.apps.display()));
    }
    stop_daemon(s, &["daemon.lock"])?;
    replace(&staging, &s.app()).map_err(|e| format!("{}: {e}", s.app().display()))?;
    check(Command::new(&s.open).arg(s.app()))?;
    hand_over(s)?;
    Ok(shown)
}

/// /Applicationsの.appを開いたあとで、そのデーモンをlaunchdに渡し、渡せたかを確かめる。e2eの後片付けも使う。
/// 開いた.appがロックを取るので、launchdが起こした方はロックを取れずに0で終わり、ジョブはrunningにならない。
/// そこで開いた方を止め、launchdから起こし直す。
pub(crate) fn hand_over(s: &Setup) -> Result<(), String> {
    let exe = s.exe();
    // 前の導入のplistが同じ場所を指していることがあるので、bootoutで外したジョブが読み込まれ直すまで待つ。
    // 読み込まれ直せば、開いた.appがロックを取り、plistを書き直して登録し終えている。
    let registered = || {
        loaded(s)
            && fs::read_to_string(s.plist())
                .is_ok_and(|p| string_after(&p, "ProgramArguments") == exe.to_str())
    };
    if wait_until(s.wait, registered) {
        stop_holder(&s.data().join("daemon.lock"), s.wait)?;
        check(Command::new(&s.launchctl).args(["kickstart", &s.service]))?;
        wait_until(s.wait, || is_running(&print(s)));
    }
    verdict(
        &print(s),
        fs::read_to_string(s.plist()).ok().as_deref(),
        &exe,
    )
}

fn uninstall(s: &Setup) -> Result<(), String> {
    stop_daemon(s, &["daemon.lock", "gui.lock"])?;
    let plist = s.plist();
    match fs::remove_file(&plist) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(io_err(&plist, e)),
        _ => {}
    }
    let app = s.app();
    if app.exists() {
        fs::remove_dir_all(&app).map_err(|e| io_err(&app, e))?;
    }
    println!("{} とログイン時の起動を消しました", app.display());
    println!("設定と状態は残しています: {}", s.data().display());
    Ok(())
}

/// launchdのジョブを外し、`.app`をopenで開いたなどlaunchdの外で動くものも止める。
fn stop_daemon(s: &Setup, locks: &[&str]) -> Result<(), String> {
    // 読み込まれていなければ失敗するが、止める相手がいないだけなので気にしない。
    let _ = Command::new(&s.launchctl)
        .args(["bootout", &s.service])
        .stderr(Stdio::null())
        .status();
    locks
        .iter()
        .try_for_each(|l| stop_holder(&s.data().join(l), s.wait))
}

/// 前の.appを脇へ移してから入れ替え、最後に消す。
fn replace(staging: &Path, app: &Path) -> io::Result<()> {
    let old = staging.with_extension("old");
    let had = app.exists();
    if had {
        fs::rename(app, &old)?;
    }
    fs::rename(staging, app)?;
    if had {
        fs::remove_dir_all(&old)?;
    }
    Ok(())
}

/// ロックを持つプロセスにSIGTERMを送り、ロックが外れるまで待つ。
fn stop_holder(lock: &Path, wait: Duration) -> Result<(), String> {
    if !held(lock) {
        return Ok(());
    }
    let text = fs::read_to_string(lock).unwrap_or_default();
    let pid = holder(&text).ok_or_else(|| {
        format!(
            "{} の持ち主の pid が読めません: {:?}",
            lock.display(),
            text.trim()
        )
    })?;
    let _ = Command::new("/bin/kill")
        .args(["-TERM", &pid.to_string()])
        .stderr(Stdio::null())
        .status();
    // デーモンはSIGTERMを扱わないので、既定の動きですぐに終わる。待つのはロックでなくそのプロセスにする。
    // launchdが起こしたデーモンが、空いたロックをすぐに取り直すことがあるため。
    if wait_until(wait, || !alive(pid)) {
        Ok(())
    } else {
        Err(format!(
            "{} を持つ wharfkeep（pid {pid}）が止まりません",
            lock.display()
        ))
    }
}

/// `kill -0`が通れば居る。ゾンビにも通るが、デーモンはxtaskの子ではないので回収待ちで残らない。
fn alive(pid: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &pid.to_string()])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|st| st.success())
}

/// ロックのファイルに書かれたpid。0や負の数を渡すと、killはプロセスグループやすべてのプロセスへ送る。
fn holder(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok().filter(|p| *p > 0)
}

/// ほかのプロセスがロックを持っているか。ファイルを作らない。
fn held(lock: &Path) -> bool {
    File::open(lock).is_ok_and(|f| matches!(f.try_lock(), Err(TryLockError::WouldBlock)))
}

fn wait_until(limit: Duration, ok: impl FnMut() -> bool) -> bool {
    crate::wait::wait_until(limit, Duration::from_millis(100), ok)
}

fn loaded(s: &Setup) -> bool {
    Command::new(&s.launchctl)
        .args(["print", &s.service])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|st| st.success())
}

fn print(s: &Setup) -> String {
    Command::new(&s.launchctl)
        .args(["print", &s.service])
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// plistの`<key>key</key>`の次にある`<string>`の中身。
pub(crate) fn string_after<'a>(xml: &'a str, key: &str) -> Option<&'a str> {
    let rest = &xml[xml.find(&format!("<key>{key}</key>"))?..];
    let rest = &rest[rest.find("<string>")? + "<string>".len()..];
    Some(&rest[..rest.find("</string>")?])
}

/// launchdのジョブ自身の状態。入れ子の`endpoints`などの`state`は見ない。
pub(crate) fn is_running(print: &str) -> bool {
    print.lines().any(|l| l.trim() == "state = running")
}

fn verdict(print: &str, plist: Option<&str>, exe: &Path) -> Result<(), String> {
    let Some(plist) = plist else {
        return Err("ログイン時の起動の plist がありません。自動起動がオフだと書かれません。メニューバーで「ログイン時に起動」をオンにしてから入れ直してください".into());
    };
    let program = string_after(plist, "ProgramArguments").unwrap_or("");
    if Some(program) != exe.to_str() {
        return Err(format!(
            "plist の ProgramArguments が {exe} でなく {program} を指しています",
            exe = exe.display()
        ));
    }
    if !is_running(print) {
        return Err("launchctl print でデーモンが running になりません".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
