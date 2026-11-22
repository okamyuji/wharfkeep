use crate::i18n::{Lang, Msg};
use eframe::egui;
use std::io;
use std::process::{Command, Stdio};

/// 自分自身を`gui`引数で別プロセスとして起動する。閉じたら描画に使ったメモリをOSへ返すため。
pub fn launch_gui() -> io::Result<()> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("gui")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // 終わった子を回収しないとゾンビとして残るため、待つだけのスレッドを置く。
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// ウィンドウを開き、閉じられるまで戻らない。
pub fn run() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("wharfkeep")
            .with_inner_size([480.0, 320.0]),
        event_loop_builder: Some(Box::new(|_builder| {
            // バンドルのInfo.plistはLSUIElement（Dockに出さない）なので、GUIだけDockとCmd+Tabに出す。
            #[cfg(target_os = "macos")]
            {
                use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
                _builder.with_activation_policy(ActivationPolicy::Regular);
            }
        })),
        ..Default::default()
    };
    eframe::run_native(
        "wharfkeep",
        options,
        Box::new(|_cc| {
            Ok(Box::new(App {
                lang: Lang::detect(),
                reloads: 0,
            }))
        }),
    )
}

struct App {
    lang: Lang,
    reloads: u32,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            if ui.button(Msg::Reload.text(self.lang)).clicked() {
                self.reloads += 1;
            }
            ui.label(format!(
                "{} {}",
                Msg::ReloadCount.text(self.lang),
                self.reloads
            ));
        });
    }
}
