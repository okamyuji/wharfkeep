//! target/wharfkeep.appをOSから操作して確かめる。メニューバーからGUIを開いて閉じる流れ、二度目の「Open Window」、
//! デーモンを止めたときのGUIの表示、状態ファイル、コンテナ画面の行のボタン、メニューからのインスタンスの停止と起動、
//! ログイン時の起動（launchd）、削除と片付け、イメージのpullからコンテナの実行と削除までを試す。
//! 先に`cargo xtask bundle --no-bump`を実行しておく。アクセシビリティと画面収録の許可が要る。
//! colimaのインスタンスdefaultを一度止めて起動し直す。終わると、自動起動を有効にしたままlaunchdからデーモンを
//! 動かした状態にし、defaultを始めの状態に戻す。/Applications/wharfkeep.appがあれば、ログイン時の起動先をそちらへ戻す。
//!
//! macOSは前面のアプリがある画面のメニューバーだけをAXに出し、ノッチなどで隠れた項目は出さない。
//! メニューバーを引く直前に、外部画面にあるアプリ（WHARFKEEP_E2E_FRONT_APP、既定はZed）を前面にする。
mod ax;
mod en;
pub mod judge;
mod probe;
mod proc;
mod resources;
mod restore;

use proc::*;

use crate::install::{is_running, string_after};
use std::fs;
use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const STEP: Duration = Duration::from_millis(200);
const OSA_LIMIT: Duration = Duration::from_secs(10);
/// 行のボタンの試験に使うコンテナ。colimaのdefaultのソケットを直接指す。
const ROW: &str = "wk-e2e-row";
/// 削除と片付け、pullと実行の試験は専用のインスタンスで行う。defaultの利用者のイメージなどを消さないため。
const ISO: &str = "wk-e2e";
const WEB: &str = "wk-e2e-web";
const WEB_IMAGE: &str = "nginx:alpine";
const WEB_PORT: u16 = 8080;
const APPLICATIONS_APP: &str = "/Applications/wharfkeep.app";

struct Ctx {
    app: PathBuf,
    exe: PathBuf,
    out: PathBuf,
    data: PathBuf,
    state: PathBuf,
    plist: PathBuf,
    domain: String,
    service: String,
    front_app: String,
    exe_re: String,
    colima_was_running: bool,
    default_sock: Option<String>,
    iso_created: AtomicBool,
    iso_sock: Mutex<Option<String>>,
    stage: Mutex<String>,
    /// 後片付けの結果。片付けている間はロックを持ち、ほかの呼び出しを待たせる。
    restored: Mutex<Option<bool>>,
}

/// 段を走らせる子プロセスの印。
const CHILD: &str = "WHARFKEEP_E2E_CHILD";

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    crate::install::no_args(args).and_then(|()| match std::env::var_os(CHILD) {
        None => supervise(),
        Some(_) => run_stages(root),
    })
}

/// 端末のCtrl-Cは前面のプロセスグループに届く。段は別のグループの子で走らせ、
/// 親が先に終わったら子が後片付けをしてから終わる。
fn supervise() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("xtask の場所が分かりません: {e}"))?;
    let status = Command::new(exe)
        .arg("e2e")
        .env(CHILD, "1")
        .process_group(0)
        .status()
        .map_err(|e| format!("e2e の子プロセスを起動できません: {e}"))?;
    judge::child_outcome(status.code())
}

fn run_stages(root: &Path) -> Result<(), String> {
    let ctx = start(root)?;
    let done = catch_unwind(AssertUnwindSafe(|| ctx.stages())).is_ok();
    let restored = ctx.restore();
    judge::outcome(done, restored, &ctx.current())
        .map(|m| println!("{m}（画面は {}）", ctx.out.display()))
}

/// 事前の確かめを通してから、見張りとcaffeinateを置く。確かめで止まるときは何も片付けない。
fn start(root: &Path) -> Result<Arc<Ctx>, String> {
    let limit = judge::limit_secs(std::env::var("E2E_LIMIT").ok().as_deref())?;
    let ctx = Arc::new(Ctx::new(root).and_then(|c| c.preflight().map(|()| c))?);
    watchdog(Arc::clone(&ctx), limit, std::os::unix::process::parent_id());
    // 実行中に画面が消えてロックされないようにする。-wで、このプロセスが終われば一緒に終わる。
    let _ = Command::new("caffeinate")
        .args(["-di", "-w", &std::process::id().to_string()])
        .spawn();
    Ok(ctx)
}

/// 全体の上限を超えたか、親（Ctrl-Cで終わる側）が消えたら、そのときの段を書いて後片付けし、失敗で終わる。
fn watchdog(ctx: Arc<Ctx>, limit: u64, parent: u32) {
    let started = std::time::Instant::now();
    std::thread::spawn(move || {
        let why = loop {
            std::thread::sleep(Duration::from_secs(1));
            let alive = std::os::unix::process::parent_id() == parent;
            if let Some(why) = judge::stop_reason(started.elapsed(), limit, alive) {
                break why;
            }
        };
        eprintln!("e2e: 失敗: {why}（段: {}）", ctx.current());
        ctx.restore();
        std::process::exit(1);
    });
}

impl Ctx {
    fn new(root: &Path) -> Result<Ctx, String> {
        let home = std::env::home_dir().ok_or("ホームディレクトリが分かりません")?;
        let uid = text(Command::new("/usr/bin/id").arg("-u")).unwrap_or_default();
        let app = root.join("target/wharfkeep.app");
        let exe = app.join("Contents/MacOS/wharfkeep");
        let data = home.join("Library/Application Support/work.okamyuji.wharfkeep");
        let domain = format!("gui/{}", uid.trim());
        let colima_was_running = colima_running();
        Ok(Ctx {
            exe_re: judge::regex_escape(&exe.to_string_lossy()),
            out: root.join("target/e2e"),
            state: data.join("state.json"),
            plist: home.join("Library/LaunchAgents/wharfkeep.plist"),
            service: format!("{domain}/wharfkeep"),
            front_app: std::env::var("WHARFKEEP_E2E_FRONT_APP").unwrap_or_else(|_| "Zed".into()),
            // defaultが止まっているとcolima statusが失敗する。その場合は下の「起動してから」で止める。
            default_sock: socket_of("default"),
            iso_created: AtomicBool::new(false),
            iso_sock: Mutex::new(None),
            stage: Mutex::new("準備".into()),
            restored: Mutex::new(None),
            colima_was_running,
            app,
            exe,
            data,
            domain,
        })
    }

    // ---- 失敗と待ち ----

    fn fail(&self, msg: &str) -> ! {
        eprintln!("e2e: 失敗: {msg}");
        self.restore();
        std::process::exit(1);
    }

    fn need(&self, ok: bool, msg: impl FnOnce() -> String) {
        if !ok {
            self.fail(&msg());
        }
    }

    /// 条件が成り立つまで繰り返す。最大`secs`秒。
    fn wait(&self, secs: u64, ok: impl FnMut() -> bool) -> bool {
        crate::wait::wait_until(Duration::from_secs(secs), STEP, ok)
    }

    /// 段の見出しを出し、上限を超えたときに書く段として覚える。
    fn say(&self, line: &str) {
        println!("{line}");
        *self.stage.lock().unwrap_or_else(|e| e.into_inner()) = line.to_string();
    }

    fn current(&self) -> String {
        self.stage.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn stages(&self) {
        self.start();
        self.en_menu();
        self.en_rows();
        self.en_dashboard_and_raise();
        self.en_daemon_stopped();
        self.en_instance();
        self.autostart();
        self.autostart_off_and_on();
        self.resources_setup();
        self.resources_samples();
        self.resources_images();
        self.resources_volumes_networks();
        self.resources_ja();
        self.web();
        self.ja();
        self.finish();
    }
}
