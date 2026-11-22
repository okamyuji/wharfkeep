use crate::gui::launch_gui;
use crate::i18n::{Lang, Msg};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder, TrayIconEvent};
use winit::application::ApplicationHandler;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::WindowId;

struct App {
    tray: Option<TrayIcon>,
    open: MenuItem,
    quit: MenuItem,
}

impl App {
    fn create_tray(&mut self) -> Result<TrayIcon, String> {
        let menu = Menu::new();
        menu.append(&self.open).map_err(|e| e.to_string())?;
        menu.append(&self.quit).map_err(|e| e.to_string())?;
        let icon = Icon::from_rgba(icon_rgba(32), 32, 32).map_err(|e| e.to_string())?;
        TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .with_tooltip("wharfkeep")
            .build()
            .map_err(|e| format!("メニューバーに項目を出せません: {e}"))
    }
}

impl ApplicationHandler<MenuEvent> for App {
    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        // macOSはイベントループが動き出してからでないとステータス項目を作れない。
        if cause == StartCause::Init {
            match self.create_tray() {
                Ok(t) => self.tray = Some(t),
                Err(e) => {
                    eprintln!("wharfkeep: {e}");
                    event_loop.exit();
                }
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, ev: MenuEvent) {
        if ev.id == self.open.id() {
            if let Err(e) = launch_gui() {
                eprintln!("wharfkeep: ウィンドウを開けません: {e}");
            }
        } else if ev.id == self.quit.id() {
            event_loop.exit();
        }
    }

    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}

    fn window_event(&mut self, _: &ActiveEventLoop, _: WindowId, _: WindowEvent) {}
}

/// 塗りつぶした丸のRGBA。画像ファイルを同梱しないため描く。
fn icon_rgba(size: u32) -> Vec<u8> {
    let c = size as f32 / 2.0;
    let r2 = (c - 1.0) * (c - 1.0);
    (0..size * size)
        .flat_map(|i| {
            let (x, y) = ((i % size) as f32 + 0.5 - c, (i / size) as f32 + 0.5 - c);
            if x * x + y * y <= r2 {
                [0x25, 0x63, 0xeb, 255]
            } else {
                [0, 0, 0, 0]
            }
        })
        .collect()
}

/// メニューバーに項目を出し、「終了」が選ばれるまで戻らない。
pub fn run() -> Result<(), String> {
    #[allow(unused_mut)]
    let mut builder = EventLoop::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    let event_loop = builder
        .build()
        .map_err(|e| format!("イベントループを作れません: {e}"))?;
    // メニューの操作はイベントとして届けて起こす。定期的に起きて確かめるとアイドル中も電力を使う。
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |ev| {
        let _ = proxy.send_event(ev);
    }));
    // アイコンのクリックやホバーは使わない。受け手を置かないと内部の無制限のチャネルに溜まり続ける。
    TrayIconEvent::set_event_handler(Some(|_| {}));
    let lang = Lang::detect();
    let mut app = App {
        tray: None,
        open: MenuItem::new(Msg::OpenWindow.text(lang), true, None),
        quit: MenuItem::new(Msg::Quit.text(lang), true, None),
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("イベントループが止まりました: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_is_square_rgba_with_transparent_corner_and_opaque_center() {
        let px = icon_rgba(32);
        assert_eq!(px.len(), 32 * 32 * 4);
        assert_eq!(&px[0..4], &[0, 0, 0, 0]);
        assert_eq!(px[(16 * 32 + 16) * 4 + 3], 255);
    }
}
