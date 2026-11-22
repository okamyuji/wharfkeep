//! メニューバーの項目。中身（どの項目を並べ、アイコンを何色にするか）は状態ファイルと同じ`Snapshot`から決める。

use super::UserEvent;
use crate::colima::InstanceStatus;
use crate::i18n::{Lang, Msg};
use crate::snapshot::{DockerState, Snapshot};
use objc2::rc::Retained;
use objc2::{
    AllocAnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel,
};
use objc2_app_kit::{
    NSBitmapFormat, NSBitmapImageRep, NSControlStateValueOn, NSDeviceRGBColorSpace, NSImage,
    NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSObject, NSSize, NSString};
use std::cell::RefCell;
use winit::event_loop::EventLoopProxy;

/// メニューの1項目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    /// 実行中なら停止、そうでなければ起動する項目。
    Instance {
        name: String,
        running: bool,
    },
    OpenWindow,
    /// チェックの有無は設定から付ける。
    StartAtLogin,
    Quit,
}

impl MenuEntry {
    pub fn title(&self, lang: Lang) -> String {
        match self {
            Self::Instance { name, running } => {
                let m = if *running {
                    Msg::StopInstance
                } else {
                    Msg::StartInstance
                };
                m.text(lang).replace("{name}", name)
            }
            Self::OpenWindow => Msg::OpenWindow.text(lang).into(),
            Self::StartAtLogin => Msg::StartAtLogin.text(lang).into(),
            Self::Quit => Msg::Quit.text(lang).into(),
        }
    }
}

/// アイコンの状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    Error,
    Normal,
    Idle,
}

impl IconState {
    /// VoiceOverが読む説明。色が見えなくても状態が分かるようにする。
    pub fn description(self, lang: Lang) -> &'static str {
        match self {
            Self::Error => Msg::IconError,
            Self::Normal => Msg::IconNormal,
            Self::Idle => Msg::IconIdle,
        }
        .text(lang)
    }

    /// RGBA。Idleは同じ青を45%の不透明度にする。
    fn color(self) -> [u8; 4] {
        match self {
            Self::Error => [0xD2, 0x66, 0x5A, 0xFF],
            Self::Normal => [0x25, 0x63, 0xEB, 0xFF],
            Self::Idle => [0x25, 0x63, 0xEB, 115],
        }
    }
}

pub fn menu_model(s: &Snapshot) -> Vec<MenuEntry> {
    s.instances
        .iter()
        .map(|i| MenuEntry::Instance {
            name: i.name.clone(),
            running: i.status == InstanceStatus::Running,
        })
        .chain([
            MenuEntry::OpenWindow,
            MenuEntry::StartAtLogin,
            MenuEntry::Quit,
        ])
        .collect()
}

pub fn icon_state(s: &Snapshot) -> IconState {
    let failed = s.docker.values().any(|d| match d {
        DockerState::Ready(sum) => sum.containers.iter().any(|c| c.failed),
        DockerState::Unavailable { .. } => false,
    });
    if failed {
        IconState::Error
    } else if s
        .instances
        .iter()
        .any(|i| i.status == InstanceStatus::Running)
    {
        IconState::Normal
    } else {
        IconState::Idle
    }
}

/// `size`×`size`の塗りつぶした丸のRGBA（乗算済みでないアルファ）。画像ファイルを同梱しないため描く。
pub fn icon_rgba(state: IconState, size: u32) -> Vec<u8> {
    let c = size as f32 / 2.0;
    let r2 = (c - 1.0) * (c - 1.0);
    (0..size * size)
        .flat_map(|i| {
            let (x, y) = ((i % size) as f32 + 0.5 - c, (i / size) as f32 + 0.5 - c);
            if x * x + y * y <= r2 {
                state.color()
            } else {
                [0, 0, 0, 0]
            }
        })
        .collect()
}

/// 表示する画像の大きさ（ポイント）。メニューバーの高さ22ポイントに収める。
const ICON_POINTS: f64 = 18.0;
/// 描く画素数。Retinaで粗く見えないよう2倍で描く。
const ICON_PIXELS: u32 = 36;

/// メニューの項目が選ばれたときの受け手。選ばれた項目をwinitのイベントとして送る。
pub(crate) struct TargetIvars {
    entries: RefCell<Vec<MenuEntry>>,
    proxy: EventLoopProxy<UserEvent>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "WharfkeepMenuTarget"]
    #[ivars = TargetIvars]
    pub(crate) struct MenuTarget;

    impl MenuTarget {
        #[unsafe(method(choose:))]
        fn choose(&self, sender: &NSMenuItem) {
            let entry = usize::try_from(sender.tag())
                .ok()
                .and_then(|i| self.ivars().entries.borrow().get(i).cloned());
            if let Some(e) = entry {
                let _ = self.ivars().proxy.send_event(UserEvent::Menu(e));
            }
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker, proxy: EventLoopProxy<UserEvent>) -> Retained<MenuTarget> {
        let this = mtm.alloc::<MenuTarget>().set_ivars(TargetIvars {
            entries: RefCell::new(Vec::new()),
            proxy,
        });
        unsafe { msg_send![super(this), init] }
    }
}

/// メニューバーの項目。メニューは`NSStatusItem.menu`に付けたままにする。
/// クリック時にだけ付ける作りではVoiceOverやAXPressから開けない。
pub(crate) struct StatusItem {
    mtm: MainThreadMarker,
    item: Retained<NSStatusItem>,
    menu: Retained<NSMenu>,
    target: Retained<MenuTarget>,
    lang: Lang,
    shown_menu: Option<(Vec<MenuEntry>, bool)>,
    shown_icon: Option<IconState>,
}

impl StatusItem {
    pub(crate) fn new(
        mtm: MainThreadMarker,
        proxy: EventLoopProxy<UserEvent>,
        lang: Lang,
    ) -> StatusItem {
        let item = NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        let menu = NSMenu::new(mtm);
        item.setMenu(Some(&menu));
        StatusItem {
            mtm,
            item,
            menu,
            target: MenuTarget::new(mtm, proxy),
            lang,
            shown_menu: None,
            shown_icon: None,
        }
    }

    /// 変わった所だけ作り直す。開いているメニューを毎周期作り直すと、選ぼうとした項目が消える。
    pub(crate) fn show(&mut self, entries: Vec<MenuEntry>, start_at_login: bool, icon: IconState) {
        if self.shown_icon != Some(icon) {
            self.set_icon(icon);
            self.shown_icon = Some(icon);
        }
        let menu = (entries, start_at_login);
        if self.shown_menu.as_ref() != Some(&menu) {
            self.rebuild_menu(&menu.0, start_at_login);
            self.shown_menu = Some(menu);
        }
    }

    fn set_icon(&self, icon: IconState) {
        let Some(button) = self.item.button(self.mtm) else {
            return;
        };
        let image = ns_image(icon);
        image.setAccessibilityDescription(Some(&NSString::from_str(icon.description(self.lang))));
        button.setImage(Some(&image));
    }

    fn rebuild_menu(&self, entries: &[MenuEntry], start_at_login: bool) {
        self.menu.removeAllItems();
        *self.target.ivars().entries.borrow_mut() = entries.to_vec();
        for (i, e) in entries.iter().enumerate() {
            if *e == MenuEntry::OpenWindow && i > 0 {
                self.menu.addItem(&NSMenuItem::separatorItem(self.mtm));
            }
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    self.mtm.alloc(),
                    &NSString::from_str(&e.title(self.lang)),
                    Some(sel!(choose:)),
                    &NSString::new(),
                )
            };
            // targetは弱い参照なので、受け手はStatusItemが持ち続ける。
            unsafe { item.setTarget(Some(&self.target)) };
            item.setTag(i as isize);
            if *e == MenuEntry::StartAtLogin && start_at_login {
                item.setState(NSControlStateValueOn);
            }
            self.menu.addItem(&item);
        }
    }
}

/// `icon_rgba`の画素から作る。テンプレート画像にすると色が消えるので、そうしない。
fn ns_image(icon: IconState) -> Retained<NSImage> {
    let size = NSSize::new(ICON_POINTS, ICON_POINTS);
    let image = NSImage::initWithSize(NSImage::alloc(), size);
    let px = ICON_PIXELS as isize;
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            px,
            px,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            NSBitmapFormat::AlphaNonpremultiplied,
            px * 4,
            32,
        )
    };
    if let Some(rep) = rep {
        let rgba = icon_rgba(icon, ICON_PIXELS);
        // 上の引数でAppKitが`px * 4 * px`バイトを確保している。
        unsafe { std::ptr::copy_nonoverlapping(rgba.as_ptr(), rep.bitmapData(), rgba.len()) };
        rep.setSize(size);
        image.addRepresentation(&rep);
    }
    image
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colima::Instance;
    use crate::snapshot::{ContainerRow, DockerSummary};
    use std::collections::BTreeMap;
    use std::time::SystemTime;

    fn instance(name: &str, status: InstanceStatus) -> Instance {
        Instance {
            name: name.into(),
            status,
            arch: "aarch64".into(),
            cpus: 2,
            memory_bytes: 1,
            disk_bytes: 1,
            runtime: "docker".into(),
        }
    }

    fn container(state: &str, failed: bool) -> ContainerRow {
        ContainerRow {
            name: format!("c-{state}"),
            state: state.into(),
            failed,
            ..Default::default()
        }
    }

    fn snap(instances: Vec<Instance>, docker: Vec<(&str, DockerState)>) -> Snapshot {
        Snapshot {
            updated_at: SystemTime::UNIX_EPOCH,
            instances,
            docker: docker
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn ready(containers: Vec<ContainerRow>) -> DockerState {
        DockerState::Ready(DockerSummary {
            containers,
            ..Default::default()
        })
    }

    #[test]
    fn menu_lists_each_instance_with_toggle() {
        let s = snap(
            vec![
                instance("default", InstanceStatus::Running),
                instance("dev", InstanceStatus::Stopped),
                instance("odd", InstanceStatus::Other("Broken".into())),
            ],
            vec![],
        );
        assert_eq!(
            menu_model(&s),
            [
                MenuEntry::Instance {
                    name: "default".into(),
                    running: true
                },
                MenuEntry::Instance {
                    name: "dev".into(),
                    running: false
                },
                MenuEntry::Instance {
                    name: "odd".into(),
                    running: false
                },
                MenuEntry::OpenWindow,
                MenuEntry::StartAtLogin,
                MenuEntry::Quit,
            ]
        );
    }

    #[test]
    fn menu_without_instances_keeps_fixed_items() {
        assert_eq!(
            menu_model(&snap(vec![], vec![])),
            [
                MenuEntry::OpenWindow,
                MenuEntry::StartAtLogin,
                MenuEntry::Quit
            ]
        );
    }

    #[test]
    fn titles_in_both_languages() {
        let stop = MenuEntry::Instance {
            name: "default".into(),
            running: true,
        };
        let start = MenuEntry::Instance {
            name: "dev".into(),
            running: false,
        };
        assert_eq!(stop.title(Lang::En), "Stop default");
        assert_eq!(stop.title(Lang::Ja), "default を停止");
        assert_eq!(start.title(Lang::En), "Start dev");
        assert_eq!(start.title(Lang::Ja), "dev を起動");
        assert_eq!(MenuEntry::OpenWindow.title(Lang::En), "Open Window");
        assert_eq!(MenuEntry::StartAtLogin.title(Lang::En), "Start at Login");
        assert_eq!(MenuEntry::StartAtLogin.title(Lang::Ja), "ログイン時に起動");
        assert_eq!(MenuEntry::Quit.title(Lang::Ja), "終了");
    }

    #[test]
    fn icon_error_when_any_container_failed() {
        let s = snap(
            vec![
                instance("default", InstanceStatus::Running),
                instance("dev", InstanceStatus::Running),
            ],
            vec![
                ("default", ready(vec![container("running", false)])),
                (
                    "dev",
                    ready(vec![container("running", false), container("dead", true)]),
                ),
            ],
        );
        assert_eq!(icon_state(&s), IconState::Error);
    }

    #[test]
    fn icon_normal_when_running_without_failures() {
        let s = snap(
            vec![
                instance("default", InstanceStatus::Running),
                instance("dev", InstanceStatus::Stopped),
            ],
            vec![
                (
                    "default",
                    ready(vec![
                        container("exited", false),
                        container("running", false),
                    ]),
                ),
                (
                    "dev",
                    DockerState::Unavailable {
                        reason: "stopped".into(),
                    },
                ),
            ],
        );
        assert_eq!(icon_state(&s), IconState::Normal);
    }

    #[test]
    fn icon_idle_when_none() {
        assert_eq!(icon_state(&snap(vec![], vec![])), IconState::Idle);
        let stopped = snap(
            vec![
                instance("default", InstanceStatus::Stopped),
                instance("odd", InstanceStatus::Other("Running?".into())),
            ],
            vec![],
        );
        assert_eq!(icon_state(&stopped), IconState::Idle);
    }

    #[test]
    fn icon_descriptions_differ_per_state_and_language() {
        let all = [IconState::Error, IconState::Normal, IconState::Idle];
        for lang in [Lang::En, Lang::Ja] {
            let d: Vec<_> = all.iter().map(|s| s.description(lang)).collect();
            assert!(d.iter().all(|t| t.starts_with("wharfkeep: ")), "{d:?}");
            assert_ne!(d[0], d[1]);
            assert_ne!(d[1], d[2]);
            assert_ne!(d[0], d[2]);
        }
        assert_eq!(
            IconState::Idle.description(Lang::En),
            "wharfkeep: no instance running"
        );
    }

    fn pixel(px: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        px[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn icon_colors_per_state() {
        let size = 36;
        let center = |s| pixel(&icon_rgba(s, size), size, size / 2, size / 2);
        assert_eq!(center(IconState::Error), [0xD2, 0x66, 0x5A, 0xFF]);
        assert_eq!(center(IconState::Normal), [0x25, 0x63, 0xEB, 0xFF]);
        assert_eq!(center(IconState::Idle), [0x25, 0x63, 0xEB, 115]);
    }

    #[test]
    fn icon_is_a_circle_with_transparent_corners() {
        let size = 36;
        let px = icon_rgba(IconState::Normal, size);
        assert_eq!(px.len(), (size * size * 4) as usize);
        for (x, y) in [(0, 0), (size - 1, 0), (0, size - 1), (size - 1, size - 1)] {
            assert_eq!(pixel(&px, size, x, y), [0, 0, 0, 0], "({x},{y})");
        }
        // 縁の中ほどは円の内側に入る。
        assert_eq!(pixel(&px, size, 1, size / 2)[3], 0xFF);
        assert_eq!(pixel(&px, size, size / 2, size - 2)[3], 0xFF);
    }
}
