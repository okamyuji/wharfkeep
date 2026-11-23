//! 画面ごとの描画。

pub mod containers;
mod dashboard;
pub mod images;
pub mod instances;
pub mod networks;
#[cfg(test)]
mod test_util;
pub mod volumes;

use super::bridge::{Bridge, Request, ResourceKind};
use super::state::{GuiState, Page};
use crate::colima::Action;
use crate::i18n::{Lang, Msg};
use crate::snapshot::DockerState;
use eframe::egui;

/// 見出し。eguiは見出しの役割を付けないので、VoiceOverが見出しとして読めるよう付ける。
pub fn heading(ui: &mut egui::Ui, text: &str) {
    let response = ui.heading(text);
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_role(egui::accesskit::Role::Heading);
        // 見出しの名前は`label`で読まれる。`Label`の役割ではテキストが`value`にしか入らない。
        node.set_label(text);
    });
}

/// ボタンなどにVoiceOverとe2eが使うAXの名前を付ける。見た目の文字（アイコン）とは別にする。
pub fn name_widget(ui: &egui::Ui, response: &egui::Response, name: &str) {
    ui.ctx().accesskit_node_builder(response.id, |node| {
        node.set_label(name);
    });
}

/// 行ごとのごみ箱ボタン。AXの名前は「削除 <名前>」で、処理中は押せない。
pub fn trash_button(ui: &mut egui::Ui, lang: Lang, name: &str, busy: bool) -> bool {
    let label = format!("{} {name}", Msg::Delete.text(lang));
    let response = ui
        .add_enabled(!busy, egui::Button::new("🗑"))
        .on_hover_text(&label)
        .on_disabled_hover_text(&label);
    name_widget(ui, &response, &label);
    response.clicked()
}

/// 開いている詳細。`text`は返事が届くまで`None`。
#[derive(Debug)]
pub struct Inspect {
    pub kind: ResourceKind,
    pub target: String,
    pub name: String,
    pub text: Option<String>,
}

impl Inspect {
    pub fn is(&self, kind: ResourceKind, target: &str) -> bool {
        self.kind == kind && self.target == target
    }
}

/// 行の「詳細」ボタン。押すと詳細を開き、inspectを頼む。
pub fn inspect_button(
    ui: &mut egui::Ui,
    state: &mut GuiState,
    bridge: &Bridge,
    lang: Lang,
    profile: &str,
    (kind, target, name): (ResourceKind, &str, &str),
) {
    let label = format!("{} {name}", Msg::RowInspect.text(lang));
    let response = ui.button("🔍").on_hover_text(&label);
    name_widget(ui, &response, &label);
    if response.clicked() {
        state.inspect = Some(Inspect {
            kind,
            target: target.to_string(),
            name: name.to_string(),
            text: None,
        });
        bridge.send(Request::InspectResource {
            profile: profile.to_string(),
            kind,
            target: target.to_string(),
        });
    }
}

/// `kind`の詳細を、画面の下に出す。
pub fn inspect_panel(ui: &mut egui::Ui, state: &mut GuiState, lang: Lang, kind: ResourceKind) {
    let Some(i) = state.inspect.as_ref().filter(|i| i.kind == kind) else {
        return;
    };
    let mut close = false;
    egui::Panel::bottom("resource-inspect")
        .resizable(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.strong(format!("{}: {}", Msg::InspectTitle.text(lang), i.name));
                close = ui.button(Msg::Close.text(lang)).clicked();
            });
            if let Some(text) = &i.text {
                egui::ScrollArea::vertical().show(ui, |ui| ui.monospace(text));
            }
        });
    if close {
        state.inspect = None;
    }
}

/// 画面上部の「片付け…」ボタン。
pub fn cleanup_button(ui: &mut egui::Ui, lang: Lang, enabled: bool) -> bool {
    ui.add_enabled(enabled, egui::Button::new(Msg::Cleanup.text(lang)))
        .clicked()
}

/// 取れなかった値は「不明」と出す。
pub fn size_text(bytes: Option<u64>, lang: Lang) -> String {
    bytes.map_or_else(|| Msg::Unknown.text(lang).to_string(), format_bytes)
}

/// ログの1行。`show_rows`は行の高さを固定して数えるので、折り返すと行が重なる。1行に切り詰め、全文はホバーで出す。
pub fn mono_line(ui: &mut egui::Ui, line: &str) {
    ui.add(egui::Label::new(egui::RichText::new(line).monospace()).truncate());
}

/// 名前付きの選択肢。値の表示はcolimaの引数と同じ文字。
pub fn choice<T: Copy + PartialEq, const N: usize>(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut T,
    all: [T; N],
    name: fn(T) -> &'static str,
) {
    egui::ComboBox::from_label(label)
        .selected_text(name(*value))
        .show_ui(ui, |ui| {
            for v in all {
                ui.selectable_value(value, v, name(v));
            }
        });
}

pub const BAR_WIDTH: f32 = 4.0;

/// 枠の左端に置く色帯の位置。
pub fn bar_rect(frame: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(frame.min, egui::pos2(frame.min.x + BAR_WIDTH, frame.max.y))
}

/// 行の左端の状態の色帯。コンテナ画面とダッシュボードで同じものを使う。
pub fn paint_bar(ui: &mut egui::Ui, tone: containers::StatusTone) {
    let height = ui.spacing().interact_size.y;
    let (bar, _) = ui.allocate_exact_size(egui::vec2(BAR_WIDTH, height), egui::Sense::hover());
    ui.painter().rect_filled(bar, 1.0, tone.color());
}

/// 1024進の読みやすい大きさ。
pub fn format_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn show(ui: &mut egui::Ui, state: &mut GuiState, bridge: &Bridge, lang: Lang) {
    heading(ui, state.page.title().text(lang));
    if state.page.needs_docker() {
        return docker_page(ui, state, bridge, lang);
    }
    if state.page == Page::Dashboard {
        // 利用者がウィンドウを縮めても、下の枠が切れずに届くようにする。
        // 一覧のページと同じ既定のidだと、スクロール位置がページをまたいで残る。
        egui::ScrollArea::vertical()
            .id_salt("dashboard")
            .show(ui, |ui| {
                no_instances(ui, state, lang);
                dashboard::show(ui, state, bridge, lang);
            });
    } else {
        instances::show(ui, state, bridge, lang);
    }
}

fn no_instances(ui: &mut egui::Ui, state: &GuiState, lang: Lang) {
    if state
        .snapshot
        .as_ref()
        .is_some_and(|s| s.instances.is_empty())
    {
        ui.label(Msg::NoInstances.text(lang));
    }
}

/// 選んだインスタンスのDockerを使う画面。つながらないときは理由と起動ボタンを出す。
fn docker_page(ui: &mut egui::Ui, state: &mut GuiState, bridge: &Bridge, lang: Lang) {
    let Some(name) = state.selected.clone() else {
        no_instances(ui, state, lang);
        return;
    };
    match state.selected_docker() {
        Some(DockerState::Ready(summary)) => {
            let summary = summary.clone();
            match state.page {
                Page::Containers => {
                    containers::show(ui, state, bridge, lang, &name, &summary.containers)
                }
                Page::Images => images::show(ui, state, bridge, lang, &name, &summary),
                Page::Volumes => volumes::show(ui, state, bridge, lang, &name, &summary.volumes),
                Page::Networks => networks::show(ui, state, bridge, lang, &name, &summary.networks),
                Page::Dashboard | Page::Instances => {}
            }
        }
        Some(DockerState::Unavailable { reason }) => {
            let reason = reason.clone();
            unavailable(ui, state, bridge, lang, &name, &reason);
        }
        None => unavailable(
            ui,
            state,
            bridge,
            lang,
            &name,
            Msg::InstanceNotRunning.text(lang),
        ),
    }
}

/// Dockerに接続できないインスタンス。理由と、インスタンスを起動するボタンを出す。
fn unavailable(
    ui: &mut egui::Ui,
    state: &mut GuiState,
    bridge: &Bridge,
    lang: Lang,
    name: &str,
    reason: &str,
) {
    ui.label(Msg::DockerUnavailable.text(lang));
    ui.label(reason);
    let idle = !state.busy.contains(name);
    if ui
        .add_enabled(idle, egui::Button::new(Msg::StartInstanceButton.text(lang)))
        .clicked()
    {
        state.busy.insert(name.to_string());
        bridge.send(Request::Colima {
            action: Action::Start,
            profile: name.to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_rect_hugs_the_left_edge_of_the_frame() {
        let frame = egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(110.0, 90.0));
        let bar = bar_rect(frame);
        assert_eq!(bar.min, frame.min);
        assert_eq!(bar.max, egui::pos2(10.0 + BAR_WIDTH, 90.0));
    }

    #[test]
    fn format_bytes_uses_1024_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1 << 20), "1.0 MiB");
        assert_eq!(format_bytes(10 << 30), "10.0 GiB");
        assert_eq!(format_bytes(3 << 40), "3.0 TiB");
        // TiBより上は単位を増やさず、数を大きくする。
        assert_eq!(format_bytes(2048 << 40), "2048.0 TiB");
    }
}
