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
    // 操作が別のインスタンスに向く名前の行は、押せても断られるだけなので出さない。
    s.instances
        .iter()
        .filter(|i| crate::colima::check_profile(&i.name).is_ok())
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

/// `ICON_PIXELS`×`ICON_PIXELS`の輸送用コンテナの形のRGBA（乗算済みでないアルファ）。画像ファイルを同梱しないため描く。
pub fn icon_rgba(state: IconState) -> Vec<u8> {
    // 36画素の格子に描く輸送用コンテナ。上下の縁と左右の柱のあいだに、縦の溝を3本入れる。
    (0..ICON_PIXELS * ICON_PIXELS)
        .flat_map(|i| {
            let (x, y) = (i % ICON_PIXELS, i / ICON_PIXELS);
            let inside = (2..34).contains(&x) && (8..28).contains(&y);
            let corner = matches!(x, 2 | 33) && matches!(y, 8 | 27);
            let rail = !(10..26).contains(&y);
            let groove = matches!(x, 9 | 10 | 17 | 18 | 25 | 26);
            if inside && !corner && (rail || !groove) {
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
        // SAFETY: `this`はset_ivarsを済ませた未初期化のMenuTargetで、NSObjectの`init`をちょうど1回呼ぶ。
        // `mtm`がメインスレッドを保証し、クラスはMainThreadOnlyで宣言している。
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
            // SAFETY: `mtm`がメインスレッドを保証する。titleとkeyEquivalentは生きているNSStringで、
            // `choose:`はMenuTargetが実装するselector。このselectorは下のsetTargetでMenuTargetに結ぶ。
            let item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    self.mtm.alloc(),
                    &NSString::from_str(&e.title(self.lang)),
                    Some(sel!(choose:)),
                    &NSString::new(),
                )
            };
            // targetは弱い参照なので、受け手はStatusItemが持ち続ける。
            // SAFETY: メインスレッドで呼ぶ（`self.mtm`）。targetはメニュー項目が弱く参照する。
            // 持ち主は`self.target`で、StatusItemが生きている間は解放されない。
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
    // SAFETY: planesにnullを渡すので、AppKitが自前で確保する。幅と高さは`px`、1画素4バイト（RGBA8）、
    // bytesPerRowは`px * 4`、bitsPerPixelは32で互いに矛盾せず、colorSpaceNameは定数のNSString。
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
        let rgba = icon_rgba(icon);
        assert_eq!(rgba.len(), (ICON_PIXELS * ICON_PIXELS * 4) as usize);
        // SAFETY: 確保した`bitmapData`は`px * 4 * px`（36 x 36 x 4）バイトで非null。`icon_rgba`は
        // 同じ`ICON_PIXELS`から同じ長さのVecを返し、別々の確保なので重ならない。`rep`はここで生きている。
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
    fn menu_omits_instances_whose_name_colima_resolves_elsewhere() {
        let s = snap(
            vec![
                instance("colima-dev", InstanceStatus::Running),
                instance("dev", InstanceStatus::Stopped),
                instance("colima", InstanceStatus::Stopped),
            ],
            vec![],
        );
        assert_eq!(
            menu_model(&s),
            [
                MenuEntry::Instance {
                    name: "dev".into(),
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
        // 溝にかからない胴の画素で色を見る。
        let panel = |s| pixel(&icon_rgba(s), ICON_PIXELS, 13, 18);
        assert_eq!(panel(IconState::Error), [0xD2, 0x66, 0x5A, 0xFF]);
        assert_eq!(panel(IconState::Normal), [0x25, 0x63, 0xEB, 0xFF]);
        assert_eq!(panel(IconState::Idle), [0x25, 0x63, 0xEB, 115]);
    }

    #[test]
    fn icon_is_a_ribbed_container() {
        let px = icon_rgba(IconState::Normal);
        assert_eq!(px.len(), (ICON_PIXELS * ICON_PIXELS * 4) as usize);
        let alpha = |x, y| pixel(&px, ICON_PIXELS, x, y)[3];
        let opaque = [
            // 箱の左右と上下の端の画素。
            (2, 18),
            (33, 18),
            (17, 8),
            (17, 27),
            // 角の隣は残る。
            (3, 8),
            (2, 9),
            (32, 27),
            (33, 26),
            // 上下の縁は溝の列でも途切れない。
            (9, 9),
            (18, 8),
            (26, 26),
            (25, 27),
            // 溝の両隣の胴。
            (8, 18),
            (11, 18),
            (16, 18),
            (19, 18),
            (24, 18),
            (27, 18),
        ];
        let transparent = [
            // 箱の外。上下に余白があり、横長に見える。
            (0, 0),
            (35, 35),
            (1, 18),
            (34, 18),
            (17, 7),
            (17, 28),
            // 角は1画素落として丸く見せる。
            (2, 8),
            (33, 8),
            (2, 27),
            (33, 27),
            // 縦の溝3本。縁のすぐ内側から始まり、縁のすぐ内側で終わる。
            (9, 10),
            (10, 18),
            (17, 18),
            (18, 25),
            (25, 10),
            (26, 25),
        ];
        for (x, y) in opaque {
            assert_eq!(alpha(x, y), 0xFF, "opaque ({x},{y})");
        }
        for (x, y) in transparent {
            assert_eq!(alpha(x, y), 0, "transparent ({x},{y})");
        }
    }
}
