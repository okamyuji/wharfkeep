//! 破壊的な操作の確認ダイアログ。承認したときだけ操作を返す。

use super::bridge::{ContainerOp, Request};
use crate::colima::Action;
use crate::i18n::{Lang, Msg};
use eframe::egui;

#[derive(Debug, Clone, PartialEq)]
pub enum PendingAction {
    Colima {
        action: Action,
        profile: String,
    },
    /// コンテナを消す。`force`なら止めてから消す。
    Container {
        profile: String,
        id: String,
        force: bool,
    },
    /// `id`はタグか、タグのないイメージのID。使用中のイメージだけ、画面で`force`を選べる。
    Image {
        profile: String,
        id: String,
        force: bool,
        in_use: bool,
        size: u64,
    },
    Volume {
        profile: String,
        name: String,
        size: Option<u64>,
    },
    Network {
        profile: String,
        id: String,
    },
    PruneVolumes {
        profile: String,
    },
    PruneNetworks {
        profile: String,
    },
}

impl PendingAction {
    pub fn into_request(self) -> Request {
        match self {
            PendingAction::Colima { action, profile } => Request::Colima { action, profile },
            PendingAction::Container { profile, id, force } => Request::Container {
                profile,
                id,
                op: ContainerOp::Remove { force },
            },
            PendingAction::Image {
                profile,
                id,
                force,
                size,
                ..
            } => Request::RemoveImage {
                profile,
                id,
                force,
                size,
            },
            PendingAction::Volume {
                profile,
                name,
                size,
            } => Request::RemoveVolume {
                profile,
                name,
                size,
            },
            PendingAction::Network { profile, id } => Request::RemoveNetwork { profile, id },
            PendingAction::PruneVolumes { profile } => Request::PruneVolumes { profile },
            PendingAction::PruneNetworks { profile } => Request::PruneNetworks { profile },
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Confirm {
    pub message: String,
    pub action: PendingAction,
}

/// ダイアログを出す。承認したときだけ`Some`を返し、承認でも取り消しでも閉じる。
/// 背景のクリックとEscapeは取り消しになる。
pub fn show(ui: &mut egui::Ui, confirm: &mut Option<Confirm>, lang: Lang) -> Option<PendingAction> {
    let pending = confirm.as_mut()?;
    let mut accepted = false;
    let mut cancelled = false;
    let response = egui::Modal::new(egui::Id::unique("wharfkeep-confirm")).show(ui.ctx(), |ui| {
        ui.label(&pending.message);
        if let PendingAction::Image {
            force,
            in_use: true,
            ..
        } = &mut pending.action
        {
            ui.checkbox(force, Msg::ForceDeleteImage.text(lang));
        }
        ui.horizontal(|ui| {
            cancelled = ui.button(Msg::Cancel.text(lang)).clicked();
            accepted = ui.button(Msg::Delete.text(lang)).clicked();
        });
    });
    if !accepted && !cancelled && !response.should_close() {
        return None;
    }
    let done = confirm.take()?;
    accepted.then_some(done.action)
}
