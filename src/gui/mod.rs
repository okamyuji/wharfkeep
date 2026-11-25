mod bridge;
pub mod confirm;
mod fonts;
mod log_buffer;
mod pages;
mod raise;
mod state;
#[cfg(test)]
mod tests;

use crate::colima::Colima;
use crate::i18n::{Lang, Msg};
use crate::paths;
use crate::terminal;
use bridge::Bridge;
use eframe::egui;
use state::{GuiState, Page, Polled, StateWatcher, daemon_stopped};
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// 状態ファイルを確かめ直す間隔。
const POLL_EVERY: Duration = Duration::from_secs(1);

/// 日本語のラベルが折り返さない幅。
const SIDEBAR_WIDTH: f32 = 220.0;

/// 警告の色。メニューバーの失敗の色と同じ珊瑚色。
pub(crate) const WARN: egui::Color32 = egui::Color32::from_rgb(0xD2, 0x66, 0x5A);

/// 自分自身を`gui`引数で別プロセスとして起動する。閉じたら描画に使ったメモリをOSへ返すため。
pub fn launch_gui() -> io::Result<()> {
    spawn_self("gui")
}

fn spawn_self(arg: &str) -> io::Result<()> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg(arg)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // 終わった子を回収しないとゾンビとして残るため、待つだけのスレッドを置く。
    std::thread::spawn(move || child.wait());
    Ok(())
}

pub use raise::raise_running;

pub(crate) use fonts::install_fonts;

/// 初めのウィンドウの大きさ。高さは、内訳が上限の行数になったダッシュボードが収まる値にする。
/// それより長い一覧は右ペインの中でスクロールする。
pub const WINDOW_SIZE: [f32; 2] = [960.0, 500.0];

fn native_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("wharfkeep")
            // 最小化したウィンドウは、2つ目の起動が前面にしても元に戻らず、Dockに残る。
            .with_minimize_button(false)
            .with_inner_size(WINDOW_SIZE),
        event_loop_builder: Some(Box::new(|_builder| {
            // バンドルのInfo.plistはLSUIElement（Dockに出さない）なので、GUIだけDockとCmd+Tabに出す。
            #[cfg(target_os = "macos")]
            {
                use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
                _builder.with_activation_policy(ActivationPolicy::Regular);
            }
        })),
        ..Default::default()
    }
}

/// ウィンドウを開き、閉じられるまで戻らない。
pub fn run() -> eframe::Result {
    let lang = Lang::detect();
    let dir = paths::data_dir()
        .ok_or_else(|| eframe::Error::AppCreation(Msg::NoHomeDir.text(lang).into()))?;
    eframe::run_native(
        "wharfkeep",
        native_options(),
        Box::new(move |cc| {
            install_fonts(&cc.egui_ctx, lang);
            let path_env = std::env::var("PATH").unwrap_or_default();
            let bin =
                paths::find_binary("colima", &path_env).unwrap_or_else(|| PathBuf::from("colima"));
            let ctx = cc.egui_ctx.clone();
            let bridge = Bridge::spawn(
                Colima::new(bin),
                lang,
                move || ctx.request_repaint(),
                std::sync::Arc::new(terminal::open_in_terminal),
            )?;
            Ok(Box::new(
                App::new(
                    lang,
                    dir.join("state.json"),
                    bridge,
                    Box::new(|| spawn_self("daemon")),
                )
                .with_preferred(std::env::var("WHARFKEEP_PROFILE").ok()),
            ))
        }),
    )
}

pub(crate) struct App {
    lang: Lang,
    state: GuiState,
    watcher: StateWatcher,
    bridge: Bridge,
    start_daemon: Box<dyn FnMut() -> io::Result<()>>,
}

impl App {
    pub(crate) fn new(
        lang: Lang,
        state_path: PathBuf,
        bridge: Bridge,
        start_daemon: Box<dyn FnMut() -> io::Result<()>>,
    ) -> App {
        App {
            lang,
            state: GuiState::new(),
            watcher: StateWatcher::new(state_path),
            bridge,
            start_daemon,
        }
    }

    pub(crate) fn with_preferred(mut self, name: Option<String>) -> App {
        self.state.preferred = name;
        self
    }

    pub(crate) fn draw(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        // 状態ファイルで選んだインスタンスが消えて選び直すときも、切り替えとして扱う。
        let before = self.state.selected.clone();
        self.poll_state();
        self.drain_replies();
        ui.ctx().request_repaint_after(POLL_EVERY);

        let stopped = daemon_stopped(self.state.snapshot.as_ref(), SystemTime::now());
        if stopped || !self.state.toasts.is_empty() {
            egui::Panel::top("notices").show(ui, |ui| {
                if stopped {
                    self.daemon_banner(ui);
                }
                self.toasts(ui);
            });
        }
        egui::Panel::left("sidebar")
            .min_size(SIDEBAR_WIDTH)
            .show(ui, |ui| self.sidebar(ui));
        if self.state.selected != before
            && let Some(request) = self.state.leave_instance()
        {
            self.bridge.send(request);
        }
        egui::CentralPanel::default().show(ui, |ui| {
            pages::show(ui, &mut self.state, &self.bridge, lang);
        });
        if let Some(action) = confirm::show(ui, &mut self.state.confirm, lang) {
            let request = self.state.start_pending(action);
            self.bridge.send(request);
        }
    }

    fn poll_state(&mut self) {
        match self.watcher.poll(Instant::now()) {
            Polled::Unchanged => {}
            Polled::Loaded(snapshot) => self.state.apply_snapshot(snapshot),
            Polled::Failed(e) => self.state.push_toast(e.message(self.lang)),
        }
    }

    fn drain_replies(&mut self) {
        while let Some(reply) = self.bridge.try_recv() {
            self.state.apply_reply(reply);
        }
    }

    fn daemon_banner(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.colored_label(WARN, Msg::DaemonNotRunning.text(self.lang));
            if ui.button(Msg::StartDaemon.text(self.lang)).clicked()
                && let Err(e) = (self.start_daemon)()
            {
                self.state
                    .push_toast(format!("{}: {e}", Msg::DaemonStartFailed.text(self.lang)));
            }
        });
    }

    fn toasts(&mut self, ui: &mut egui::Ui) {
        let mut dismissed = None;
        for (i, toast) in self.state.toasts.iter().enumerate() {
            ui.horizontal(|ui| {
                if toast.info {
                    ui.label(&toast.message);
                } else {
                    ui.colored_label(WARN, &toast.message);
                }
                if ui.button(Msg::Dismiss.text(self.lang)).clicked() {
                    dismissed = Some(i);
                }
            });
        }
        if let Some(i) = dismissed {
            self.state.toasts.remove(i);
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let lang = self.lang;
        let names: Vec<String> = self
            .state
            .snapshot
            .iter()
            .flat_map(|s| s.instances.iter().map(|i| i.name.clone()))
            .collect();
        // 名前を持たない選択肢はVoiceOverで読めないので、ラベルで名前を付ける。
        egui::ComboBox::from_label(Msg::InstanceLabel.text(lang))
            .selected_text(self.state.selected.as_deref().unwrap_or(""))
            .show_ui(ui, |ui| {
                for name in names {
                    ui.selectable_value(&mut self.state.selected, Some(name.clone()), name);
                }
            });
        ui.separator();
        for page in Page::ALL {
            ui.selectable_value(&mut self.state.page, page, page.title().text(lang));
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}
