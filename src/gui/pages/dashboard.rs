//! ダッシュボード。集計の枠とディスク使用量。

use super::containers::{StatusTone, status_tone};
use super::format_bytes;
use super::instances::status_text as instance_status_text;
use crate::colima::InstanceStatus;
use crate::gui::bridge::Bridge;
use crate::gui::state::{GuiState, Page};
use crate::i18n::{Lang, Msg};
use crate::snapshot::{DiskUsage, DockerState, DockerSummary, Snapshot};
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Totals {
    pub instances: usize,
    pub running: usize,
    pub cpus: u32,
    pub memory_bytes: u64,
}

/// 全インスタンスの合計。止まっているものもCPUとメモリに数える。
pub fn totals(s: &Snapshot) -> Totals {
    s.instances.iter().fold(Totals::default(), |t, i| Totals {
        instances: t.instances + 1,
        running: t.running + usize::from(i.status == InstanceStatus::Running),
        cpus: t.cpus + i.cpus,
        memory_bytes: t.memory_bytes + i.memory_bytes,
    })
}

/// 動いていれば緑、止まっていれば灰、それ以外（状態が読めない）は珊瑚色。
pub fn instance_tone(status: &InstanceStatus) -> StatusTone {
    match status {
        InstanceStatus::Running => StatusTone::Running,
        InstanceStatus::Stopped => StatusTone::Stopped,
        InstanceStatus::Other(_) => StatusTone::Failed,
    }
}

/// 選んだインスタンスのコンテナを、実行中、一時停止、停止、起動失敗に数える。
pub fn container_counts(sum: &DockerSummary) -> [(StatusTone, usize); CONTAINER_LINES] {
    let mut counts = [
        (StatusTone::Running, 0),
        (StatusTone::Paused, 0),
        (StatusTone::Stopped, 0),
        (StatusTone::Failed, 0),
    ];
    for row in &sum.containers {
        let tone = status_tone(row);
        if let Some(c) = counts.iter_mut().find(|(t, _)| *t == tone) {
            c.1 += 1;
        }
    }
    counts
}

fn tone_text(tone: StatusTone, lang: Lang) -> &'static str {
    match tone {
        StatusTone::Running => Msg::StateRunning,
        StatusTone::Paused => Msg::StatePaused,
        StatusTone::Stopped => Msg::StateStopped,
        StatusTone::Failed => Msg::StateFailed,
    }
    .text(lang)
}

const COLUMNS: usize = 3;
/// コンテナの枠の内訳の行数（実行中、一時停止、停止、失敗）。
const CONTAINER_LINES: usize = 4;
/// インスタンスの枠の内訳の上限。超えた分は最後の1行で「ほか N 件」とまとめる。枠の高さもこれで止まる。
const MAX_INSTANCE_LINES: usize = 5;
const CARD_PAD_X: i8 = 14;
const CARD_PAD_Y: i8 = 8;

/// 格子の1枠の大きさ。全枠をこの幅と高さにそろえる。
#[derive(Debug, Clone, Copy)]
struct Cell {
    width: f32,
    height: f32,
}

/// 3列の等幅。高さは、見出しと`max_lines`行の内訳が`row`の高さで収まる大きさ。
fn cell_size(available_width: f32, gap: f32, max_lines: usize, row: f32) -> Cell {
    Cell {
        width: (available_width - gap * (COLUMNS - 1) as f32) / COLUMNS as f32,
        height: row * (1 + max_lines) as f32 + f32::from(CARD_PAD_Y) * 2.0 + 4.0,
    }
}

/// 枠の1行の高さ。日本語は補いのフォントで描くので英語より高い。実際の文字で測る。
fn row_height<'a>(ui: &egui::Ui, texts: impl IntoIterator<Item = &'a str>) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let color = ui.visuals().text_color();
    let text_height = texts
        .into_iter()
        .map(|t| {
            ui.painter()
                .layout_no_wrap(t.to_string(), font.clone(), color)
                .size()
                .y
        })
        .fold(0.0, f32::max);
    text_height + ui.spacing().item_spacing.y
}

pub fn show(ui: &mut egui::Ui, state: &mut GuiState, _bridge: &Bridge, lang: Lang) {
    let Some(snapshot) = &state.snapshot else {
        return;
    };
    let summary = match state.selected_docker() {
        Some(DockerState::Ready(sum)) => Some(sum.clone()),
        _ => None,
    };
    let t = totals(snapshot);
    let resources = Msg::DashResources
        .text(lang)
        .replace("{cpu}", &t.cpus.to_string())
        .replace("{mem}", &format_bytes(t.memory_bytes));
    let (instances_header, instances_lines, instances_tone) = instances_card(snapshot, lang);
    let containers = summary.as_ref().map(|sum| containers_card(sum, lang));
    let mut texts = vec![instances_header.as_str(), resources.as_str()];
    texts.extend(instances_lines.iter().map(String::as_str));
    if let Some((header, lines, _)) = &containers {
        texts.push(header);
        texts.extend(lines.iter().map(String::as_str));
    }
    let cell = cell_size(
        ui.available_width(),
        ui.spacing().item_spacing.x,
        instances_lines.len().max(CONTAINER_LINES),
        row_height(ui, texts),
    );
    let mut goto = None;
    ui.horizontal(|ui| {
        if tone_card(
            ui,
            cell,
            &instances_header,
            &instances_lines,
            instances_tone,
        ) {
            goto = Some(Page::Instances);
        }
        if tone_card(ui, cell, &resources, &[], None) {
            goto = Some(Page::Instances);
        }
        if let Some((header, lines, tone)) = &containers
            && tone_card(ui, cell, header, lines, *tone)
        {
            goto = Some(Page::Containers);
        }
    });
    if let Some(sum) = &summary {
        ui.horizontal(|ui| {
            for (page, text) in resource_cards(sum, lang) {
                if tone_card(ui, cell, &text, &[], None) {
                    goto = Some(page);
                }
            }
        });
        let total = Cell {
            width: cell.width * COLUMNS as f32 + ui.spacing().item_spacing.x * (COLUMNS - 1) as f32,
            height: 0.0,
        };
        if let Some(page) = disk_usage(ui, total, lang, &sum.disk) {
            goto = Some(page);
        }
    }
    if let Some(page) = goto {
        state.page = page;
    }
}

/// 枠の色帯を、中の状態のうち最も急ぐものの色にする。失敗、実行中、一時停止、それ以外（停止）の順。
pub fn card_tone(tones: impl IntoIterator<Item = StatusTone>) -> Option<StatusTone> {
    let tones: Vec<StatusTone> = tones.into_iter().collect();
    [StatusTone::Failed, StatusTone::Running, StatusTone::Paused]
        .into_iter()
        .find(|t| tones.contains(t))
        .or_else(|| (!tones.is_empty()).then_some(StatusTone::Stopped))
}

/// 見出しを左上に置き、内訳をその下に並べる枠。色帯は左端。枠のどこを押しても`true`になる。
fn tone_card(
    ui: &mut egui::Ui,
    cell: Cell,
    header: &str,
    lines: &[String],
    tone: Option<StatusTone>,
) -> bool {
    let frame = card_frame(ui, cell, |ui| {
        ui.strong(header);
        // 長い名前で枠が伸びると格子がずれるので、1行に切り詰める。全文はホバーとAXの名前で出す。
        for line in lines {
            let response = ui.add(egui::Label::new(line).truncate());
            super::name_widget(ui, &response, line);
        }
    });
    let rect = frame.response.rect;
    let click = ui.interact(rect, frame.response.id.with("card"), egui::Sense::click());
    click.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), header));
    if let Some(tone) = tone {
        let bar = super::bar_rect(rect);
        ui.painter().rect_filled(bar, 1.0, tone.color());
    }
    click
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

/// `cell`の大きさに固定した枠。中身が小さくても、大きくならない。
fn card_frame<R>(
    ui: &mut egui::Ui,
    cell: Cell,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    let frame = egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::symmetric(CARD_PAD_X, CARD_PAD_Y));
    let margin = frame.total_margin();
    frame.show(ui, |ui| {
        let inner = egui::vec2(
            cell.width - margin.sum().x,
            (cell.height - margin.sum().y).max(0.0),
        );
        ui.set_min_size(inner);
        ui.set_max_width(inner.x);
        ui.vertical(content).inner
    })
}

fn instances_card(s: &Snapshot, lang: Lang) -> (String, Vec<String>, Option<StatusTone>) {
    let t = totals(s);
    let header = Msg::DashInstances
        .text(lang)
        .replace("{n}", &t.instances.to_string())
        .replace("{r}", &t.running.to_string());
    let shown = if s.instances.len() > MAX_INSTANCE_LINES {
        MAX_INSTANCE_LINES - 1
    } else {
        s.instances.len()
    };
    let mut lines: Vec<String> = s.instances[..shown]
        .iter()
        .map(|i| format!("{}: {}", i.name, instance_status_text(&i.status, lang)))
        .collect();
    if shown < s.instances.len() {
        let rest = (s.instances.len() - shown).to_string();
        lines.push(Msg::DashMoreInstances.text(lang).replace("{n}", &rest));
    }
    let tone = card_tone(s.instances.iter().map(|i| instance_tone(&i.status)));
    (header, lines, tone)
}

/// 件数のある区分のうち、最も急ぐものの色。
fn counts_tone(counts: &[(StatusTone, usize)]) -> Option<StatusTone> {
    card_tone(counts.iter().filter(|(_, n)| *n > 0).map(|(t, _)| *t))
}

fn containers_card(sum: &DockerSummary, lang: Lang) -> (String, Vec<String>, Option<StatusTone>) {
    let counts = container_counts(sum);
    let total: usize = counts.iter().map(|(_, n)| n).sum();
    let header = Msg::DashContainers
        .text(lang)
        .replace("{n}", &total.to_string())
        .replace("{r}", &counts[0].1.to_string());
    let lines: Vec<String> = counts
        .iter()
        .map(|(tone, n)| format!("{}: {n}", tone_text(*tone, lang)))
        .collect();
    let tone = counts_tone(&counts);
    (header, lines, tone)
}

fn resource_cards(sum: &DockerSummary, lang: Lang) -> [(Page, String); 3] {
    let count = |m: Msg, n: usize| m.text(lang).replace("{n}", &n.to_string());
    [
        (Page::Images, count(Msg::DashImages, sum.images.len())),
        (Page::Volumes, count(Msg::DashVolumes, sum.volumes.len())),
        (Page::Networks, count(Msg::DashNetworks, sum.networks.len())),
    ]
}

/// ディスク使用量の区分と、押したときに開く画面。ビルドキャッシュはイメージ画面で扱う。
/// 各項目は（画面, 区分の名前, 容量）。
pub fn disk_parts(d: &DiskUsage, lang: Lang) -> [(Page, String, String); 4] {
    let part = |page, m: Msg, n: Option<u64>| {
        let name = m.text(lang).replace("{size}", "").trim().to_string();
        (page, name, super::size_text(n, lang))
    };
    [
        part(Page::Images, Msg::DiskImages, d.images),
        part(Page::Containers, Msg::DiskContainers, d.containers),
        part(Page::Volumes, Msg::DiskVolumes, d.volumes),
        part(Page::Images, Msg::DiskBuildCache, d.build_cache),
    ]
}

/// 区分の名前と容量を1つにした文。
#[cfg(test)]
pub fn disk_lines(d: &DiskUsage, lang: Lang) -> [(Page, String); 4] {
    disk_parts(d, lang).map(|(page, name, size)| (page, format!("{name} {size}")))
}

/// 3列ぶんの幅の1枠。区分の名前は左、容量は右にそろえる。区分を押すと、その画面へ移る。
fn disk_usage(ui: &mut egui::Ui, width: Cell, lang: Lang, disk: &DiskUsage) -> Option<Page> {
    let mut goto = None;
    card_frame(ui, width, |ui| {
        ui.strong(Msg::DiskUsageTitle.text(lang));
        for (page, name, size) in disk_parts(disk, lang) {
            let row = ui.available_width();
            let (rect, click) = ui.allocate_exact_size(
                egui::vec2(row, ui.spacing().interact_size.y),
                egui::Sense::click(),
            );
            let label = format!("{name} {size}");
            click.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), &label)
            });
            let font = egui::TextStyle::Body.resolve(ui.style());
            let color = ui.visuals().text_color();
            let painter = ui.painter();
            painter.text(
                rect.left_center(),
                egui::Align2::LEFT_CENTER,
                name,
                font.clone(),
                color,
            );
            painter.text(
                rect.right_center(),
                egui::Align2::RIGHT_CENTER,
                size,
                font,
                color,
            );
            if click
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                goto = Some(page);
            }
        }
    });
    goto
}

#[cfg(test)]
mod tests {
    use super::super::test_util::{harness, harness_sized, instance, state_with};
    use super::*;
    use crate::colima::InstanceStatus;
    use crate::snapshot::{ContainerRow, ImageRow, NetworkRow, VolumeRow};
    use eframe::egui::accesskit::Role;
    use egui_kittest::kittest::Queryable;
    use std::collections::BTreeMap;
    use std::time::SystemTime;

    #[test]
    fn dashboard_totals() {
        let s = Snapshot {
            updated_at: SystemTime::now(),
            instances: vec![
                instance("a", InstanceStatus::Running, 4, 8),
                instance("b", InstanceStatus::Stopped, 2, 2),
            ],
            docker: BTreeMap::new(),
        };
        assert_eq!(
            totals(&s),
            Totals {
                instances: 2,
                running: 1,
                cpus: 6,
                memory_bytes: 10 << 30
            }
        );
        let empty = Snapshot {
            instances: vec![],
            ..s
        };
        assert_eq!(totals(&empty), Totals::default());
    }

    fn summary() -> DockerSummary {
        let container = |state: &str| ContainerRow {
            state: state.into(),
            ..Default::default()
        };
        DockerSummary {
            containers: vec![
                container("running"),
                container("exited"),
                container("running"),
            ],
            images: vec![ImageRow::default(); 4],
            volumes: vec![VolumeRow::default(); 5],
            networks: vec![NetworkRow::default(); 6],
            disk: DiskUsage {
                images: Some(3 << 30),
                containers: Some(2048),
                volumes: None,
                build_cache: Some(0),
            },
        }
    }

    #[test]
    fn cards_show_the_counts_in_english() {
        let (h, _ends) = harness(Lang::En, state_with(summary(), Page::Dashboard));
        for label in [
            "Instances: 1 (1 running)",
            "CPU 4 / memory 8.0 GiB",
            "Containers: 3 (2 running)",
            "Images: 4",
            "Volumes: 5",
            "Networks: 6",
        ] {
            h.get_by_role_and_label(Role::Button, label);
        }
    }

    #[test]
    fn cards_show_the_counts_in_japanese() {
        let (h, _ends) = harness(Lang::Ja, state_with(summary(), Page::Dashboard));
        for label in [
            "インスタンス: 1（実行中 1）",
            "CPU 4 / メモリ 8.0 GiB",
            "コンテナ: 3（実行中 2）",
            "イメージ: 4",
            "ボリューム: 5",
            "ネットワーク: 6",
        ] {
            h.get_by_role_and_label(Role::Button, label);
        }
    }

    #[test]
    fn a_card_opens_its_page() {
        let cases = [
            ("Instances: 1 (1 running)", Page::Instances),
            ("CPU 4 / memory 8.0 GiB", Page::Instances),
            ("Containers: 3 (2 running)", Page::Containers),
            ("Images: 4", Page::Images),
            ("Volumes: 5", Page::Volumes),
            ("Networks: 6", Page::Networks),
        ];
        for (label, page) in cases {
            let (mut h, _ends) = harness(Lang::En, state_with(summary(), Page::Dashboard));
            h.get_by_role_and_label(Role::Button, label).click();
            h.run_steps(4);
            assert_eq!(h.state().0.page, page, "{label}");
        }
    }

    #[test]
    fn disk_usage_says_unknown_for_values_it_could_not_read() {
        let (h, _ends) = harness(Lang::En, state_with(summary(), Page::Dashboard));
        h.get_by_label("Disk usage");
        h.get_by_label("Images 3.0 GiB");
        h.get_by_label("Containers 2.0 KiB");
        h.get_by_label("Volumes unknown");
        h.get_by_label("Build cache 0 B");
    }

    #[test]
    fn disk_lines_follow_the_language() {
        let d = DiskUsage {
            images: Some(1 << 20),
            ..Default::default()
        };
        let texts: Vec<String> = disk_lines(&d, Lang::Ja).into_iter().map(|l| l.1).collect();
        assert_eq!(
            texts,
            [
                "イメージ 1.0 MiB",
                "コンテナ 不明",
                "ボリューム 不明",
                "ビルドキャッシュ 不明"
            ]
        );
    }

    #[test]
    fn disk_lines_navigate() {
        for (label, page) in [
            ("Images 3.0 GiB", Page::Images),
            ("Containers 2.0 KiB", Page::Containers),
            ("Volumes unknown", Page::Volumes),
            ("Build cache 0 B", Page::Images),
        ] {
            let (mut h, _ends) = harness(Lang::En, state_with(summary(), Page::Dashboard));
            h.get_by_role_and_label(Role::Button, label).click();
            h.run_steps(4);
            assert_eq!(h.state().0.page, page, "{label}");
        }
        let (h, _ends) = harness(Lang::En, state_with(summary(), Page::Dashboard));
        assert!(h.query_by_label_contains("Prune").is_none());
        assert!(h.query_by_label_contains("prune").is_none());
    }

    #[test]
    fn without_docker_only_the_instance_cards_show() {
        let mut state = state_with(summary(), Page::Dashboard);
        let mut snapshot = state.snapshot.clone().unwrap();
        snapshot.docker.insert(
            "dev".into(),
            DockerState::Unavailable {
                reason: "stopped".into(),
            },
        );
        state.apply_snapshot(Some(snapshot));
        let (h, _ends) = harness(Lang::En, state);
        h.get_by_role_and_label(Role::Button, "Instances: 1 (1 running)");
        assert!(
            h.query_by_role_and_label(Role::Button, "Containers: 0 (0 running)")
                .is_none()
        );
        assert!(h.query_by_label("Disk usage").is_none());
    }

    #[test]
    fn without_a_snapshot_nothing_is_drawn() {
        let (h, _ends) = harness(Lang::En, crate::gui::state::GuiState::new());
        assert!(h.query_by_label_contains("Instances:").is_none());
    }

    #[test]
    fn instance_tone_by_status() {
        assert_eq!(instance_tone(&InstanceStatus::Running), StatusTone::Running);
        assert_eq!(instance_tone(&InstanceStatus::Stopped), StatusTone::Stopped);
        assert_eq!(
            instance_tone(&InstanceStatus::Other("Broken".into())),
            StatusTone::Failed
        );
    }

    fn rows(specs: &[(&str, bool)]) -> DockerSummary {
        DockerSummary {
            containers: specs
                .iter()
                .map(|(state, failed)| ContainerRow {
                    state: (*state).into(),
                    failed: *failed,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn dashboard_container_counts() {
        let sum = rows(&[
            ("running", false),
            ("running", false),
            ("paused", false),
            ("exited", true),
            ("exited", false),
            ("created", false),
            ("running", true),
        ]);
        assert_eq!(
            container_counts(&sum),
            [
                (StatusTone::Running, 2),
                (StatusTone::Paused, 1),
                (StatusTone::Stopped, 2),
                (StatusTone::Failed, 2),
            ]
        );
    }

    #[test]
    fn the_containers_card_counts_the_selected_instance_only() {
        let mut state = state_with(rows(&[("running", false)]), Page::Dashboard);
        let mut snapshot = state.snapshot.clone().unwrap();
        snapshot
            .instances
            .push(instance("other", InstanceStatus::Running, 2, 2));
        snapshot.docker.insert(
            "other".into(),
            DockerState::Ready(rows(&[
                ("exited", false),
                ("exited", true),
                ("running", false),
            ])),
        );
        state.apply_snapshot(Some(snapshot));
        let (h, _ends) = harness(Lang::En, state);
        h.get_by_role_and_label(Role::Button, "Containers: 1 (1 running)");
        h.get_by_label("Stopped: 0");
        h.get_by_label("Failed: 0");
    }

    #[test]
    fn card_tone_takes_most_urgent() {
        use StatusTone::*;
        assert_eq!(card_tone([]), None);
        assert_eq!(card_tone([Stopped, Running, Failed, Paused]), Some(Failed));
        assert_eq!(card_tone([Stopped, Paused, Running]), Some(Running));
        assert_eq!(card_tone([Stopped, Paused]), Some(Paused));
        assert_eq!(card_tone([Stopped, Stopped]), Some(Stopped));
    }

    #[test]
    fn counts_tone_ignores_empty_categories() {
        use StatusTone::*;
        let counts = |r, p, s, f| [(Running, r), (Paused, p), (Stopped, s), (Failed, f)];
        assert_eq!(counts_tone(&counts(0, 0, 0, 0)), None);
        assert_eq!(counts_tone(&counts(0, 1, 5, 0)), Some(Paused));
        assert_eq!(counts_tone(&counts(0, 0, 2, 0)), Some(Stopped));
        assert_eq!(counts_tone(&counts(3, 1, 0, 0)), Some(Running));
        assert_eq!(counts_tone(&counts(3, 0, 0, 1)), Some(Failed));
    }

    #[test]
    fn dashboard_cards_hold_breakdown() {
        for (lang, names, counts) in [
            (
                Lang::En,
                ["dev: Running"],
                ["Running: 1", "Paused: 1", "Stopped: 1", "Failed: 1"],
            ),
            (
                Lang::Ja,
                ["dev: 実行中"],
                ["実行中: 1", "一時停止中: 1", "停止中: 1", "失敗: 1"],
            ),
        ] {
            let sum = rows(&[
                ("running", false),
                ("paused", false),
                ("exited", false),
                ("exited", true),
            ]);
            let (mut h, _ends) = harness(lang, state_with(sum, Page::Dashboard));
            for label in names.iter().chain(counts.iter()) {
                // 同じ文言が枠の外にもう1つ出ていれば、ここで複数見つかって失敗する。
                h.get_by_label(label);
            }
            let header = if lang == Lang::En {
                "Containers: 4 (1 running)"
            } else {
                "コンテナ: 4（実行中 1）"
            };
            h.get_by_role_and_label(Role::Button, header).click();
            h.run_steps(4);
            assert_eq!(h.state().0.page, Page::Containers);
        }
    }

    // 1枠の幅で折り返すと5行を超え、枠の高さをそろえられなくなる長さ。
    const LONG: &str = "a-very-long-instance-name-that-does-not-fit-in-one-third-of-the-window-and-would-wrap-onto-many-lines-if-the-card-let-it-wrap-instead-of-truncating";

    /// `name`のインスタンスだけがあり、それを選んだ状態。
    fn named_state(name: &str) -> crate::gui::state::GuiState {
        let mut state = state_with(summary(), Page::Dashboard);
        let mut snapshot = state.snapshot.clone().unwrap();
        snapshot.instances = vec![instance(name, InstanceStatus::Running, 4, 8)];
        let docker = snapshot.docker.remove("dev").unwrap();
        snapshot.docker = BTreeMap::from([(name.to_string(), docker)]);
        state.selected = None;
        state.apply_snapshot(Some(snapshot));
        state
    }

    fn grid_labels(lang: Lang) -> ([[&'static str; 3]; 2], &'static str) {
        match lang {
            Lang::En => (
                [
                    [
                        "Instances: 1 (1 running)",
                        "CPU 4 / memory 8.0 GiB",
                        "Containers: 3 (2 running)",
                    ],
                    ["Images: 4", "Volumes: 5", "Networks: 6"],
                ],
                "Images 3.0 GiB",
            ),
            Lang::Ja => (
                [
                    [
                        "インスタンス: 1（実行中 1）",
                        "CPU 4 / メモリ 8.0 GiB",
                        "コンテナ: 3（実行中 2）",
                    ],
                    ["イメージ: 4", "ボリューム: 5", "ネットワーク: 6"],
                ],
                "イメージ 3.0 GiB",
            ),
        }
    }

    #[test]
    fn the_fullest_dashboard_fits_the_initial_window_height() {
        let height = crate::gui::WINDOW_SIZE[1];
        for (lang, prefix) in [(Lang::En, "Build cache "), (Lang::Ja, "ビルドキャッシュ ")]
        {
            // 内訳が上限の行数になるよう、インスタンスを上限より1つ多く置く。
            let mut state = named_state("dev");
            let mut snapshot = state.snapshot.clone().unwrap();
            snapshot.instances = (0..=MAX_INSTANCE_LINES)
                .map(|i| instance(&format!("inst-{i}"), InstanceStatus::Running, 1, 1))
                .collect();
            snapshot.instances[0].name = "dev".into();
            state.apply_snapshot(Some(snapshot));
            let (h, _ends) =
                super::super::test_util::harness_with_size(lang, state, [740.0, height]);
            let last = h.get_by(|n| n.label().is_some_and(|l| l.starts_with(prefix)));
            assert!(last.rect().max.y <= height, "{lang:?} {:?}", last.rect());
        }
    }

    #[test]
    fn a_short_window_scrolls_the_dashboard_without_carrying_it_to_other_pages() {
        let height = 300.0;
        let mut state = named_state("dev");
        let mut snapshot = state.snapshot.clone().unwrap();
        if let Some(DockerState::Ready(sum)) = snapshot.docker.get_mut("dev") {
            for (i, v) in sum.volumes.iter_mut().enumerate() {
                v.name = format!("data-{i}");
            }
        }
        state.apply_snapshot(Some(snapshot));
        let (mut h, _ends) =
            super::super::test_util::harness_with_size(Lang::En, state, [740.0, height]);
        let last = |h: &egui_kittest::Harness<'_, _>| {
            h.get_by(|n| n.label().is_some_and(|l| l.starts_with("Build cache ")))
                .rect()
        };
        assert!(last(&h).max.y > height);
        h.get_by(|n| n.label().is_some_and(|l| l.starts_with("Build cache ")))
            .scroll_to_me();
        h.run_steps(4);
        assert!(last(&h).max.y <= height, "{:?}", last(&h));
        h.state_mut().0.page = Page::Volumes;
        h.run_steps(4);
        // 一覧が先頭から見えている。ダッシュボードのスクロール量が残ると、1行目が窓の上へ隠れる。
        let heading = h.get_by_label("Volumes").rect();
        let row = h.get_by_label("Delete data-0").rect();
        assert!(row.min.y > heading.max.y, "{row:?} {heading:?}");
    }

    #[test]
    fn dashboard_cards_share_one_grid() {
        for (lang, width, name) in [
            (Lang::En, 900.0, "dev"),
            (Lang::Ja, 900.0, "dev"),
            (Lang::En, 900.0, LONG),
            (Lang::Ja, 900.0, LONG),
            (Lang::En, 740.0, LONG),
            (Lang::Ja, 740.0, LONG),
        ] {
            let case = format!("{lang:?} {width} {name}");
            let (h, _ends) = harness_sized(lang, named_state(name), width);
            let (grid, disk_label) = grid_labels(lang);
            let rects: Vec<Vec<egui::Rect>> = grid
                .iter()
                .map(|r| r.iter().map(|l| rect_of(&h, l)).collect())
                .collect();
            let first = rects[0][0];
            // 高さは、内訳が4行のコンテナの枠に合わせた値になる。最後の行まで枠の中に収まる。
            let last = h.get_by_label(if lang == Lang::En {
                "Failed: 0"
            } else {
                "失敗: 0"
            });
            assert!(
                rects[0][2].contains_rect(last.rect()),
                "{case} {:?}",
                last.rect()
            );
            for r in rects.iter().flatten() {
                assert!(
                    (r.width() - first.width()).abs() < 0.5,
                    "{case} 幅 {r:?} {first:?}"
                );
                assert!(
                    (r.height() - first.height()).abs() < 0.5,
                    "{case} 高さ {r:?} {first:?}"
                );
            }
            for col in 0..3 {
                assert!(
                    (rects[0][col].min.x - rects[1][col].min.x).abs() < 0.5,
                    "{case} 列{col}"
                );
                assert!(rects[1][col].min.y > rects[0][col].max.y, "{case} 段{col}");
            }
            for row in &rects {
                assert!(
                    row[0].max.x < row[1].min.x && row[1].max.x < row[2].min.x,
                    "{case}"
                );
                let gap = row[1].min.x - row[0].max.x;
                assert!(
                    (row[2].min.x - row[1].max.x - gap).abs() < 0.5,
                    "{case} 列の間隔"
                );
            }
            // ディスクの枠は3列ぶん。行の左右の余白が同じなら、枠の両端は格子の両端と一致する。
            let disk = rect_of(&h, disk_label);
            let left_pad = disk.min.x - rects[0][0].min.x;
            let right_pad = rects[0][2].max.x - disk.max.x;
            assert!(left_pad > 0.0 && left_pad < 24.0, "{case} {left_pad}");
            assert!(
                (left_pad - right_pad).abs() < 0.5,
                "{case} {left_pad} {right_pad}"
            );
            assert!(disk.min.y > rects[1][0].max.y, "{case}");
        }
    }

    #[test]
    fn a_long_breakdown_line_stays_on_one_line_inside_its_card() {
        let (mut h, _ends) = harness_sized(Lang::En, named_state(LONG), 740.0);
        let full = format!("{LONG}: Running");
        let card = rect_of(&h, "Instances: 1 (1 running)");
        let line = h.get_by_label(&full).rect();
        let one_line = h.get_by_label("Running: 2").rect();
        assert!((line.height() - one_line.height()).abs() < 0.5, "{line:?}");
        assert!(line.max.x <= card.max.x, "{line:?} {card:?}");
        // 全文はホバーで出る。
        h.get_by_label(&full).hover();
        h.run_steps(4);
        assert_eq!(h.query_all_by_label(&full).count(), 2);
    }

    #[test]
    fn the_instance_card_lists_at_most_five_lines() {
        for (lang, more, first) in [
            (Lang::En, "and 3 more", "i1: Running"),
            (Lang::Ja, "ほか 3 件", "i1: 実行中"),
        ] {
            let (h, _ends) = harness(lang, instances_state(7));
            h.get_by_label(first);
            h.get_by_label_contains("i4: ");
            assert!(h.query_by_label_contains("i5: ").is_none());
            h.get_by_label(more);
            let header = if lang == Lang::En {
                "Instances: 7 (7 running)"
            } else {
                "インスタンス: 7（実行中 7）"
            };
            // 枠の高さは5行ぶんで止まり、最後の行まで中に収まる。
            let card = rect_of(&h, header);
            assert!(card.contains_rect(h.get_by_label(more).rect()), "{card:?}");
            let (five, _ends) = harness(lang, instances_state(5));
            let five_header = header.replace('7', "5");
            let five_height = rect_of(&five, &five_header).height();
            assert!(
                (card.height() - five_height).abs() < 0.5,
                "{card:?} {five_height}"
            );
        }
    }

    /// `n`個の実行中のインスタンス。Dockerの中身は`i1`だけにある。
    fn instances_state(n: usize) -> crate::gui::state::GuiState {
        let mut state = state_with(summary(), Page::Dashboard);
        let mut snapshot = state.snapshot.clone().unwrap();
        let docker = snapshot.docker.remove("dev").unwrap();
        snapshot.instances = (1..=n)
            .map(|i| instance(&format!("i{i}"), InstanceStatus::Running, 1, 1))
            .collect();
        snapshot.docker = BTreeMap::from([("i1".to_string(), docker)]);
        state.selected = None;
        state.apply_snapshot(Some(snapshot));
        state
    }

    #[test]
    fn five_instances_are_all_listed() {
        let (h, _ends) = harness(Lang::En, instances_state(5));
        h.get_by_label("i5: Running");
        assert!(h.query_by_label_contains("more").is_none());
    }

    fn rect_of(
        h: &egui_kittest::Harness<'static, super::super::test_util::Fixture>,
        label: &str,
    ) -> egui::Rect {
        h.get_by_role_and_label(Role::Button, label).rect()
    }

    #[test]
    fn disk_parts_split_the_name_from_the_size() {
        let d = DiskUsage {
            images: Some(1 << 20),
            build_cache: Some(0),
            ..Default::default()
        };
        let parts = |lang| -> Vec<(Page, String, String)> { disk_parts(&d, lang).into() };
        assert_eq!(
            parts(Lang::En),
            [
                (Page::Images, "Images".to_string(), "1.0 MiB".to_string()),
                (
                    Page::Containers,
                    "Containers".to_string(),
                    "unknown".to_string()
                ),
                (Page::Volumes, "Volumes".to_string(), "unknown".to_string()),
                (Page::Images, "Build cache".to_string(), "0 B".to_string()),
            ]
        );
        assert_eq!(parts(Lang::Ja)[3].1, "ビルドキャッシュ");
    }

    #[test]
    fn row_height_is_the_tallest_text_plus_the_item_spacing() {
        let got = std::rc::Rc::new(std::cell::Cell::new((0.0, 0.0, 0.0)));
        let out = got.clone();
        let mut h = egui_kittest::Harness::builder()
            .with_context_setup(|ctx| crate::gui::install_fonts(ctx, Lang::En))
            .build_ui(move |ui| {
                let font = egui::TextStyle::Body.resolve(ui.style());
                let color = ui.visuals().text_color();
                let text = ui
                    .painter()
                    .layout_no_wrap("Abc".into(), font, color)
                    .size()
                    .y;
                out.set((
                    row_height(ui, ["", "Abc"]),
                    text,
                    ui.spacing().item_spacing.y,
                ));
            });
        h.run();
        let (row, text, spacing) = got.get();
        assert!(spacing > 1.0 && text > 1.0);
        assert_eq!(row, text + spacing);
    }

    #[test]
    fn cell_size_splits_the_width_into_three_columns() {
        let c = cell_size(916.0, 8.0, 4, 22.0);
        assert_eq!(c.width, 300.0);
        assert_eq!(c.height, 22.0 * 5.0 + 16.0 + 4.0);
        assert_eq!(cell_size(100.0, 0.0, 0, 22.0).width, 100.0 / 3.0);
        assert_eq!(cell_size(0.0, 5.0, 0, 22.0).width, -10.0 / 3.0);
        assert_eq!(cell_size(0.0, 0.0, 1, 22.0).height, 44.0 + 20.0);
        assert_eq!(cell_size(0.0, 0.0, 1, 30.0).height, 60.0 + 20.0);
    }

    #[test]
    fn the_instance_card_opens_the_instances_page() {
        let (mut h, _ends) = harness(Lang::En, state_with(summary(), Page::Dashboard));
        h.get_by_role_and_label(Role::Button, "Instances: 1 (1 running)")
            .click();
        h.run_steps(4);
        assert_eq!(h.state().0.page, Page::Instances);
    }
}
