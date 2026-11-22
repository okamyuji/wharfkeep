//! ボリューム画面。一覧、作成、行ごとのごみ箱、未使用の整理。

use super::{cleanup_button, inspect_button, inspect_panel, size_text, trash_button};
use crate::docker::check_name;
use crate::gui::bridge::{Bridge, Cleanup, Request, ResourceKind, create_volume_key, volume_key};
use crate::gui::confirm::{Confirm, PendingAction};
use crate::gui::state::GuiState;
use crate::i18n::{Lang, Msg};
use crate::snapshot::VolumeRow;
use eframe::egui;

#[derive(Debug, Default)]
pub struct View {
    pub name: String,
}

/// 片付けで消える（未使用の無名）ボリュームの件数と、容量が分かるものの合計。
pub fn prunable_summary(rows: &[VolumeRow]) -> (usize, u64) {
    rows.iter()
        .filter(|v| v.prunable)
        .fold((0, 0), |(n, bytes), v| {
            (n + 1, bytes + v.size_bytes.unwrap_or(0))
        })
}

pub fn show(
    ui: &mut egui::Ui,
    state: &mut GuiState,
    bridge: &Bridge,
    lang: Lang,
    profile: &str,
    rows: &[VolumeRow],
) {
    create_form(ui, state, bridge, lang, profile);
    let (count, bytes) = prunable_summary(rows);
    let cleaning = state.busy_ops.contains(&Cleanup::Volumes.key(profile));
    if cleanup_button(ui, lang, count > 0 && !cleaning) {
        state.confirm = Some(Confirm {
            message: Msg::ConfirmPruneVolumes
                .text(lang)
                .replace("{n}", &count.to_string())
                .replace("{size}", &super::format_bytes(bytes)),
            action: PendingAction::PruneVolumes {
                profile: profile.to_string(),
            },
        });
    }
    inspect_panel(ui, state, lang, ResourceKind::Volume);
    if rows.is_empty() {
        ui.label(Msg::NoVolumes.text(lang));
        return;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("volume-rows")
            .striped(true)
            .num_columns(5)
            .show(ui, |ui| {
                for row in rows {
                    row_ui(ui, state, bridge, lang, profile, row);
                    ui.end_row();
                }
            });
    });
}

fn row_ui(
    ui: &mut egui::Ui,
    state: &mut GuiState,
    bridge: &Bridge,
    lang: Lang,
    profile: &str,
    row: &VolumeRow,
) {
    let busy = state.busy_ops.contains(&volume_key(&row.name));
    ui.horizontal(|ui| {
        if trash_button(ui, lang, &row.name, busy) {
            state.confirm = Some(Confirm {
                message: Msg::ConfirmDeleteVolume
                    .text(lang)
                    .replace("{name}", &row.name),
                action: PendingAction::Volume {
                    profile: profile.to_string(),
                    name: row.name.clone(),
                    size: row.size_bytes,
                },
            });
        }
        let target = (ResourceKind::Volume, row.name.as_str(), row.name.as_str());
        inspect_button(ui, state, bridge, lang, profile, target);
    });
    ui.horizontal(|ui| {
        ui.strong(&row.name);
        if busy {
            ui.spinner();
        }
    });
    ui.weak(&row.driver);
    ui.label(size_text(row.size_bytes, lang));
    ui.weak(&row.mountpoint);
}

fn create_form(
    ui: &mut egui::Ui,
    state: &mut GuiState,
    bridge: &Bridge,
    lang: Lang,
    profile: &str,
) {
    let key = create_volume_key(profile);
    let busy = state.busy_ops.contains(&key);
    let name = state.volumes.name.trim().to_string();
    let check = check_name(&name);
    ui.horizontal(|ui| {
        let caption = ui.label(Msg::NewVolume.text(lang));
        ui.add(egui::TextEdit::singleline(&mut state.volumes.name).desired_width(260.0))
            .labelled_by(caption.id);
        let create = ui.add_enabled(
            !busy && check.is_ok(),
            egui::Button::new(Msg::Create.text(lang)),
        );
        if create.clicked() {
            state.busy_ops.insert(key);
            bridge.send(Request::CreateVolume {
                profile: profile.to_string(),
                name: name.clone(),
            });
        }
    });
    if let Err(e) = &check
        && !name.is_empty()
    {
        ui.colored_label(crate::gui::WARN, e.message(lang));
    }
}

#[cfg(test)]
mod tests;
