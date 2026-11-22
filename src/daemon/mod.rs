//! 常駐するデーモン。監視は別スレッドのtokioで回し、AppKitのスレッドはwinitのイベントで結果を受けるだけにする。

pub mod autostart;
pub(crate) mod monitor;
pub mod tray;

use crate::colima::{self, Action, Colima};
use crate::gui::launch_gui;
use crate::i18n::{Lang, Msg};
use crate::paths;
use crate::snapshot::{self, Snapshot};
use objc2::MainThreadMarker;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::runtime::Handle;
use tokio::sync::Notify;
use tray::{MenuEntry, StatusItem, icon_state, menu_model};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::WindowId;

/// 監視のスレッドやメニューからAppKitのスレッドへ送るもの。
#[derive(Debug)]
pub enum UserEvent {
    Snapshot(Box<Snapshot>),
    Menu(MenuEntry),
}

struct App {
    lang: Lang,
    proxy: EventLoopProxy<UserEvent>,
    status: Option<StatusItem>,
    snapshot: Snapshot,
    start_at_login: bool,
    settings_path: PathBuf,
    rt: Handle,
    colima: Colima,
    wake: Arc<Notify>,
}

impl App {
    fn refresh(&mut self) {
        if let Some(st) = &mut self.status {
            st.show(
                menu_model(&self.snapshot),
                self.start_at_login,
                icon_state(&self.snapshot),
            );
        }
    }

    fn on_menu(&mut self, event_loop: &ActiveEventLoop, entry: MenuEntry) {
        match entry {
            MenuEntry::Instance { name, running } => {
                let action = if running { Action::Stop } else { Action::Start };
                match colima::simple_args(action, &name) {
                    Ok(args) => {
                        self.rt.spawn(monitor::run_action(
                            self.colima.clone(),
                            args,
                            self.wake.clone(),
                            self.lang,
                        ));
                    }
                    Err(e) => eprintln!(
                        "wharfkeep: {}: {}",
                        Msg::TrayActionFailed.text(self.lang),
                        e.message(self.lang)
                    ),
                }
            }
            MenuEntry::OpenWindow => {
                if let Err(e) = launch_gui() {
                    eprintln!("wharfkeep: {}: {e}", Msg::OpenWindowFailed.text(self.lang));
                }
            }
            MenuEntry::StartAtLogin => {
                self.start_at_login = !self.start_at_login;
                let (on, path, lang) = (self.start_at_login, self.settings_path.clone(), self.lang);
                // launchctlを呼ぶので、AppKitのスレッドでは待たない。
                self.rt.spawn_blocking(move || {
                    if let Err(e) =
                        autostart::save_settings(&path, &autostart::Settings { autostart: on })
                    {
                        eprintln!(
                            "wharfkeep: {}: {}: {e}",
                            Msg::SettingsSaveFailed.text(lang),
                            path.display()
                        );
                    }
                    apply_autostart(on, lang);
                });
                self.refresh();
            }
            MenuEntry::Quit => event_loop.exit(),
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn new_events(&mut self, _event_loop: &ActiveEventLoop, cause: StartCause) {
        // macOSはイベントループが動き出してからでないとステータス項目を作れない。
        if cause == StartCause::Init {
            let mtm = MainThreadMarker::new().expect("winit calls handlers on the main thread");
            self.status = Some(StatusItem::new(mtm, self.proxy.clone(), self.lang));
            self.refresh();
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, ev: UserEvent) {
        match ev {
            UserEvent::Snapshot(s) => {
                self.snapshot = *s;
                self.refresh();
            }
            UserEvent::Menu(entry) => self.on_menu(event_loop, entry),
        }
    }

    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

/// 設定に合わせてログイン時の起動を登録するか外す。失敗しても動き続ける。
/// `.app` の外（cargo で動かした開発ビルド）からは、登録先を書き換えない。
fn apply_autostart(on: bool, lang: Lang) {
    let r = std::env::current_exe()
        .map_err(|e| e.to_string())
        .and_then(|exe| {
            if autostart::in_app_bundle(&exe) {
                autostart::apply(&exe, on)
            } else {
                Ok(())
            }
        });
    if let Err(e) = r {
        eprintln!("wharfkeep: {}: {e}", Msg::AutostartFailed.text(lang));
    }
}

/// メニューバーに項目を出し、「終了」が選ばれるまで戻らない。
pub fn run() -> Result<(), String> {
    let lang = Lang::detect();
    let dir = paths::data_dir().ok_or_else(|| Msg::NoHomeDir.text(lang).to_string())?;
    if let Err(e) = snapshot::remove_stale_temps(&dir) {
        eprintln!("wharfkeep: {}: {e}", Msg::TempCleanupFailed.text(lang));
    }
    let settings_path = dir.join("settings.json");
    let start_at_login = autostart::load_settings(&settings_path).autostart;

    #[allow(unused_mut)]
    let mut builder = EventLoop::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    let event_loop = builder
        .build()
        .map_err(|e| format!("{}: {e}", Msg::EventLoopCreateFailed.text(lang)))?;
    // 更新は監視のスレッドからイベントで届く。定期的に起きて確かめるとアイドル中も電力を使う。
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();

    let runtime_err = |e: std::io::Error| format!("{}: {e}", Msg::RuntimeFailed.text(lang));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(runtime_err)?;
    let path_env = std::env::var("PATH").unwrap_or_default();
    let bin = paths::find_binary("colima", &path_env).unwrap_or_else(|| PathBuf::from("colima"));
    let colima = Colima::new(bin);
    let wake = Arc::new(Notify::new());
    let rt_handle = rt.handle().clone();
    {
        let (colima, wake, proxy) = (colima.clone(), wake.clone(), proxy.clone());
        let state_path = dir.join("state.json");
        std::thread::Builder::new()
            .name("monitor".into())
            .spawn(move || {
                apply_autostart(start_at_login, lang);
                rt.block_on(monitor::run_loop(
                    colima,
                    state_path,
                    wake,
                    lang,
                    move |s| {
                        let _ = proxy.send_event(UserEvent::Snapshot(Box::new(s)));
                    },
                ));
            })
            .map_err(runtime_err)?;
    }

    let mut app = App {
        lang,
        proxy,
        status: None,
        snapshot: Snapshot {
            updated_at: SystemTime::now(),
            instances: Vec::new(),
            docker: Default::default(),
        },
        start_at_login,
        settings_path,
        rt: rt_handle,
        colima,
        wake,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("{}: {e}", Msg::EventLoopStopped.text(lang)))
}
