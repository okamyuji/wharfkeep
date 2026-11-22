//! ネットワーク画面。一覧、作成、行ごとのごみ箱、未使用の整理。

use super::{cleanup_button, inspect_button, inspect_panel, trash_button};
use crate::docker::actions::NetDriver;
use crate::gui::bridge::{Bridge, Cleanup, Request, ResourceKind, create_network_key, network_key};
use crate::gui::confirm::{Confirm, PendingAction};
use crate::gui::state::GuiState;
use crate::i18n::{Lang, Msg};
use crate::snapshot::NetworkRow;
use eframe::egui;

#[derive(Debug)]
pub struct View {
    pub name: String,
    pub driver: NetDriver,
}

impl Default for View {
    fn default() -> Self {
        View {
            name: String::new(),
            driver: NetDriver::Bridge,
        }
    }
}

pub fn show(
    ui: &mut egui::Ui,
    state: &mut GuiState,
    bridge: &Bridge,
    lang: Lang,
    profile: &str,
    rows: &[NetworkRow],
) {
    create_form(ui, state, bridge, lang, profile);
    let cleaning = state.busy_ops.contains(&Cleanup::Networks.key(profile));
    if cleanup_button(ui, lang, !cleaning) {
        state.confirm = Some(Confirm {
            message: Msg::ConfirmPruneNetworks.text(lang).to_string(),
            action: PendingAction::PruneNetworks {
                profile: profile.to_string(),
            },
        });
    }
    inspect_panel(ui, state, lang, ResourceKind::Network);
    if rows.is_empty() {
        ui.label(Msg::NoNetworks.text(lang));
        return;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("network-rows")
            .striped(true)
            .num_columns(4)
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
    row: &NetworkRow,
) {
    let busy = state.busy_ops.contains(&network_key(&row.id));
    ui.horizontal(|ui| {
        if row.builtin {
            ui.weak(Msg::BuiltinNetwork.text(lang));
        } else if trash_button(ui, lang, &row.name, busy) {
            // 名前でなくIDを渡す。`my net`のような、名前の検査に通らない名前でも作れるため。
            state.confirm = Some(Confirm {
                message: Msg::ConfirmDeleteNetwork
                    .text(lang)
                    .replace("{name}", &row.name),
                action: PendingAction::Network {
                    profile: profile.to_string(),
                    id: row.id.clone(),
                },
            });
        }
        let target = (ResourceKind::Network, row.id.as_str(), row.name.as_str());
        inspect_button(ui, state, bridge, lang, profile, target);
    });
    ui.horizontal(|ui| {
        ui.strong(&row.name);
        if busy {
            ui.spinner();
        }
    });
    ui.weak(&row.driver);
    ui.weak(row.id.chars().take(12).collect::<String>());
}

fn create_form(
    ui: &mut egui::Ui,
    state: &mut GuiState,
    bridge: &Bridge,
    lang: Lang,
    profile: &str,
) {
    let key = create_network_key(profile);
    let busy = state.busy_ops.contains(&key);
    let name = state.networks.name.trim().to_string();
    ui.horizontal(|ui| {
        let caption = ui.label(Msg::NewNetwork.text(lang));
        ui.add(egui::TextEdit::singleline(&mut state.networks.name).desired_width(200.0))
            .labelled_by(caption.id);
        egui::ComboBox::from_label(Msg::FieldDriver.text(lang))
            .selected_text(state.networks.driver.as_str())
            .show_ui(ui, |ui| {
                for driver in NetDriver::ALL {
                    ui.selectable_value(&mut state.networks.driver, driver, driver.as_str());
                }
            });
        let create = ui.add_enabled(
            !busy && !name.is_empty(),
            egui::Button::new(Msg::Create.text(lang)),
        );
        if create.clicked() {
            state.busy_ops.insert(key);
            bridge.send(Request::CreateNetwork {
                profile: profile.to_string(),
                name: name.clone(),
                driver: state.networks.driver,
            });
        }
    });
}

#[cfg(test)]
mod tests;
