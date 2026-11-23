//! インスタンス画面。起動、停止、再起動、削除、作成、シェル。

use super::dashboard::instance_tone;
use super::{choice, format_bytes, name_widget, paint_bar};
use crate::colima::{Action, Arch, CreateSpec, Instance, InstanceStatus, MountType, VmType};
use crate::gui::bridge::{Bridge, Request};
use crate::gui::confirm::{Confirm, PendingAction};
use crate::gui::state::GuiState;
use crate::i18n::{Lang, Msg};
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstanceAction {
    Start,
    Stop,
    Restart,
    Shell,
    Delete,
}

impl InstanceAction {
    fn label(self) -> Msg {
        match self {
            InstanceAction::Start => Msg::RowStart,
            InstanceAction::Stop => Msg::RowStop,
            InstanceAction::Restart => Msg::RowRestart,
            InstanceAction::Shell => Msg::RowShell,
            InstanceAction::Delete => Msg::Delete,
        }
    }
}

fn instance_actions(status: &InstanceStatus) -> Vec<InstanceAction> {
    use InstanceAction::*;
    match status {
        InstanceStatus::Running => vec![Stop, Restart, Shell, Delete],
        _ => vec![Start, Delete],
    }
}

pub(super) fn status_text(status: &InstanceStatus, lang: Lang) -> String {
    match status {
        InstanceStatus::Running => Msg::StateRunning.text(lang).to_string(),
        InstanceStatus::Stopped => Msg::StateStopped.text(lang).to_string(),
        InstanceStatus::Other(s) => s.clone(),
    }
}

/// 作成フォームの入力。仮想化の方式、アーキテクチャ、マウントは、このMacの標準から始める。
#[derive(Debug)]
pub struct View {
    pub name: String,
    pub cpus: u32,
    pub memory_gib: f32,
    pub disk_gib: u32,
    pub vm_type: VmType,
    pub arch: Arch,
    pub mount_type: MountType,
}

impl Default for View {
    fn default() -> Self {
        View {
            name: String::new(),
            cpus: 2,
            memory_gib: 4.0,
            disk_gib: 60,
            vm_type: VmType::Vz,
            arch: Arch::host(),
            mount_type: MountType::Virtiofs,
        }
    }
}

impl View {
    pub fn spec(&self) -> CreateSpec {
        CreateSpec {
            profile: self.name.trim().to_string(),
            cpus: self.cpus,
            memory_gib: self.memory_gib,
            disk_gib: self.disk_gib,
            vm_type: self.vm_type,
            arch: self.arch,
            mount_type: self.mount_type,
        }
    }
}

fn perform(
    state: &mut GuiState,
    bridge: &Bridge,
    lang: Lang,
    instance: &Instance,
    action: InstanceAction,
) {
    let name = instance.name.clone();
    let colima = |action| Request::Colima {
        action,
        profile: name.clone(),
    };
    match action {
        InstanceAction::Shell => bridge.send(Request::Shell {
            profile: name.clone(),
            container: None,
        }),
        InstanceAction::Delete => {
            state.confirm = Some(Confirm {
                message: Msg::ConfirmDeleteInstance
                    .text(lang)
                    .replace("{name}", &name),
                action: PendingAction::Colima {
                    action: Action::Delete,
                    profile: name.clone(),
                },
            });
        }
        InstanceAction::Start | InstanceAction::Stop | InstanceAction::Restart => {
            let colima_action = match action {
                InstanceAction::Start => Action::Start,
                InstanceAction::Stop => Action::Stop,
                _ => Action::Restart,
            };
            state.begin_instance_op(&name);
            bridge.send(colima(colima_action));
        }
    }
}

pub fn show(ui: &mut egui::Ui, state: &mut GuiState, bridge: &Bridge, lang: Lang) {
    let Some(snapshot) = &state.snapshot else {
        return;
    };
    let instances = snapshot.instances.clone();
    if instances.is_empty() {
        ui.label(Msg::NoInstances.text(lang));
    }
    for instance in &instances {
        instance_row(ui, state, bridge, lang, instance);
    }
    ui.separator();
    create_form(ui, state, bridge, lang);
    if !state.instance_log.is_empty() {
        ui.separator();
        ui.strong(Msg::OutputTitle.text(lang));
        let height = ui.text_style_height(&egui::TextStyle::Monospace);
        egui::ScrollArea::vertical()
            .id_salt("instance-log")
            .stick_to_bottom(true)
            .show_rows(ui, height, state.instance_log.len(), |ui, range| {
                for line in state
                    .instance_log
                    .lines()
                    .skip(range.start)
                    .take(range.len())
                {
                    super::mono_line(ui, line);
                }
            });
    }
}

fn instance_row(
    ui: &mut egui::Ui,
    state: &mut GuiState,
    bridge: &Bridge,
    lang: Lang,
    instance: &Instance,
) {
    let busy = state.busy.contains(&instance.name);
    let mut clicked = None;
    ui.push_id(&instance.name, |ui| {
        ui.horizontal(|ui| {
            paint_bar(ui, instance_tone(&instance.status));
            ui.label(status_text(&instance.status, lang));
            ui.strong(&instance.name);
            ui.weak(format!(
                "{} / {} CPU / {} / {} / {}",
                instance.arch,
                instance.cpus,
                format_bytes(instance.memory_bytes),
                format_bytes(instance.disk_bytes),
                instance.runtime
            ));
            if busy {
                ui.spinner();
            }
            for action in instance_actions(&instance.status) {
                let name = format!("{} {}", action.label().text(lang), instance.name);
                let response = ui.add_enabled(!busy, egui::Button::new(action.label().text(lang)));
                name_widget(ui, &response, &name);
                if response.clicked() {
                    clicked = Some(action);
                }
            }
        });
    });
    if let Some(action) = clicked {
        perform(state, bridge, lang, instance, action);
    }
}

fn create_form(ui: &mut egui::Ui, state: &mut GuiState, bridge: &Bridge, lang: Lang) {
    ui.strong(Msg::NewInstance.text(lang));
    let form = &mut state.instances;
    ui.horizontal(|ui| {
        let caption = ui.label(Msg::FieldName.text(lang));
        ui.add(egui::TextEdit::singleline(&mut form.name).desired_width(160.0))
            .labelled_by(caption.id);
        // 数値の欄にも名前を付ける。付けないとVoiceOverは数だけを読む。
        let caption = ui.label(Msg::FieldCpus.text(lang));
        ui.add(egui::DragValue::new(&mut form.cpus).range(1..=64))
            .labelled_by(caption.id);
        let caption = ui.label(Msg::FieldMemory.text(lang));
        ui.add(egui::DragValue::new(&mut form.memory_gib).range(1.0..=256.0))
            .labelled_by(caption.id);
        let caption = ui.label(Msg::FieldDisk.text(lang));
        ui.add(egui::DragValue::new(&mut form.disk_gib).range(1..=2048))
            .labelled_by(caption.id);
    });
    let form = &mut state.instances;
    ui.horizontal(|ui| {
        choice(
            ui,
            Msg::FieldVmType.text(lang),
            &mut form.vm_type,
            VmType::ALL,
            VmType::as_str,
        );
        choice(
            ui,
            Msg::FieldArch.text(lang),
            &mut form.arch,
            Arch::ALL,
            Arch::as_str,
        );
        choice(
            ui,
            Msg::FieldMountType.text(lang),
            &mut form.mount_type,
            MountType::ALL,
            MountType::as_str,
        );
    });
    let spec = state.instances.spec();
    let valid = crate::colima::start_args(&spec);
    // 名前が空の間は、まだ何も言わない。
    if let Err(e) = &valid
        && !spec.profile.is_empty()
    {
        ui.colored_label(crate::gui::WARN, e.message(lang));
    }
    // colimaは既存の名前で`start`すると、そのVMの設定を書き換える。
    let exists = state.snapshot.as_ref().is_some_and(|s| {
        s.instances
            .iter()
            .any(|i| crate::colima::same_instance(&spec.profile, &i.name))
    });
    if exists {
        ui.colored_label(crate::gui::WARN, Msg::InstanceExists.text(lang));
    }
    let idle = !state.busy.contains(&spec.profile);
    let create = ui.add_enabled(
        idle && valid.is_ok() && !exists,
        egui::Button::new(Msg::Create.text(lang)),
    );
    if create.clicked() {
        state.begin_instance_op(&spec.profile);
        bridge.send(Request::Create(spec));
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::{harness, instance, requests, state_with};
    use super::*;
    use crate::gui::bridge::Reply;
    use crate::gui::state::Page;
    use crate::snapshot::{DockerState, DockerSummary, Snapshot};
    use egui_kittest::kittest::{NodeT, Queryable};
    use std::collections::BTreeMap;
    use std::time::SystemTime;

    fn two_instances() -> GuiState {
        let mut state = state_with(DockerSummary::default(), Page::Instances);
        state.apply_snapshot(Some(Snapshot {
            updated_at: SystemTime::now(),
            instances: vec![
                instance("dev", InstanceStatus::Running, 4, 8),
                instance("old", InstanceStatus::Stopped, 2, 2),
            ],
            docker: BTreeMap::from([
                (
                    "dev".to_string(),
                    DockerState::Ready(DockerSummary::default()),
                ),
                (
                    "old".to_string(),
                    DockerState::Unavailable {
                        reason: "stopped".into(),
                    },
                ),
            ]),
        }));
        state
    }

    fn colima(action: Action, profile: &str) -> Request {
        Request::Colima {
            action,
            profile: profile.into(),
        }
    }

    #[test]
    fn rows_show_status_and_resources() {
        let (h, _ends) = harness(Lang::En, two_instances());
        h.get_by_label("Running");
        h.get_by_label("Stopped");
        h.get_by_label("aarch64 / 4 CPU / 8.0 GiB / 60.0 GiB / docker");
        let (h, _ends) = harness(Lang::Ja, two_instances());
        h.get_by_label("実行中");
        h.get_by_label("停止中");
    }

    #[test]
    fn other_status_is_shown_as_is() {
        assert_eq!(
            status_text(&InstanceStatus::Other("Broken".into()), Lang::En),
            "Broken"
        );
    }

    #[test]
    fn running_instance_offers_stop_restart_shell_delete() {
        let (h, _ends) = harness(Lang::En, two_instances());
        for name in ["Stop dev", "Restart dev", "Shell dev", "Delete dev"] {
            h.get_by_label(name);
        }
        assert!(h.query_by_label("Start dev").is_none());
        h.get_by_label("Start old");
        h.get_by_label("Delete old");
        assert!(h.query_by_label("Stop old").is_none());
    }

    #[test]
    fn instance_buttons_send_colima_requests_and_mark_busy() {
        for (label, action, profile) in [
            ("Stop dev", Action::Stop, "dev"),
            ("Restart dev", Action::Restart, "dev"),
            ("Start old", Action::Start, "old"),
        ] {
            let (mut h, mut ends) = harness(Lang::En, two_instances());
            h.state_mut().0.instance_log.push("previous".into());
            h.get_by_label(label).click();
            h.run_steps(4);
            assert_eq!(requests(&mut ends), [colima(action, profile)], "{label}");
            assert!(h.state().0.busy.contains(profile));
            assert!(h.state().0.instance_log.is_empty());
            assert!(h.get_by_label(label).accesskit_node().is_disabled());
        }
    }

    #[test]
    fn instance_shell_button_asks_for_a_shell() {
        let (mut h, mut ends) = harness(Lang::En, two_instances());
        h.get_by_label("Shell dev").click();
        h.run_steps(4);
        assert_eq!(
            requests(&mut ends),
            [Request::Shell {
                profile: "dev".into(),
                container: None
            }]
        );
    }

    #[test]
    fn instance_delete_goes_through_the_confirmation() {
        let (mut h, mut ends) = harness(Lang::En, two_instances());
        h.get_by_label("Delete old").click();
        h.run_steps(4);
        let confirm = h.state().0.confirm.clone().unwrap();
        assert!(confirm.message.contains("old"));
        assert!(requests(&mut ends).is_empty());
        h.get_by_label("Delete").click();
        h.run_steps(4);
        assert_eq!(requests(&mut ends), [colima(Action::Delete, "old")]);
        assert!(h.state().0.busy.contains("old"));
    }

    #[test]
    fn create_form_blocks_invalid_profile() {
        let (mut h, mut ends) = harness(Lang::En, two_instances());
        h.state_mut().0.instances.name = "a b".into();
        h.run_steps(4);
        assert!(h.get_by_label("Create").accesskit_node().is_disabled());
        let reason = crate::colima::ColimaError::InvalidProfile("a b".into()).message(Lang::En);
        h.get_by_label(&reason);
        h.get_by_label("Create").click();
        h.run_steps(4);
        assert!(requests(&mut ends).is_empty());
    }

    #[test]
    fn create_form_blocks_a_name_that_already_exists() {
        for (lang, create, reason) in [
            (
                Lang::En,
                "Create",
                "An instance with this name already exists",
            ),
            (Lang::Ja, "作成", "同じ名前のインスタンスがあります"),
        ] {
            for name in ["old", "colima-old", "OLD"] {
                let (mut h, mut ends) = harness(lang, two_instances());
                h.state_mut().0.instances.name = name.into();
                h.run_steps(4);
                assert!(
                    h.get_by_label(create).accesskit_node().is_disabled(),
                    "{name}"
                );
                h.get_by_label(reason);
                h.get_by_label(create).click();
                h.run_steps(4);
                assert!(requests(&mut ends).is_empty(), "{name}");
            }
        }
    }

    #[test]
    fn create_form_reason_is_in_japanese() {
        let (mut h, _ends) = harness(Lang::Ja, two_instances());
        h.state_mut().0.instances.name = "a b".into();
        h.run_steps(4);
        assert!(h.get_by_label("作成").accesskit_node().is_disabled());
        h.get_by_label_contains("プロフィール名が不正");
    }

    #[test]
    fn empty_name_blocks_creation_without_a_complaint() {
        let (h, _ends) = harness(Lang::En, two_instances());
        assert!(h.get_by_label("Create").accesskit_node().is_disabled());
        assert!(h.query_by_label_contains("invalid").is_none());
    }

    #[test]
    fn valid_form_sends_create_once() {
        let (mut h, mut ends) = harness(Lang::En, two_instances());
        h.state_mut().0.instances.name = " fresh ".into();
        h.run_steps(4);
        h.get_by_label("Create").click();
        h.run_steps(4);
        let sent = requests(&mut ends);
        let [Request::Create(spec)] = sent.as_slice() else {
            panic!("{sent:?}");
        };
        assert_eq!(spec.profile, "fresh");
        assert_eq!((spec.cpus, spec.memory_gib, spec.disk_gib), (2, 4.0, 60));
        assert!(h.state().0.busy.contains("fresh"));
        assert!(h.get_by_label("Create").accesskit_node().is_disabled());
        h.state_mut().0.apply_reply(Reply::ColimaDone {
            profile: "fresh".into(),
            error: None,
        });
        h.run_steps(4);
        assert!(!h.get_by_label("Create").accesskit_node().is_disabled());
    }

    #[test]
    fn the_form_starts_with_this_macs_defaults() {
        let spec = View::default().spec();
        assert_eq!(spec.vm_type, VmType::Vz);
        assert_eq!(spec.mount_type, MountType::Virtiofs);
        let host = if cfg!(target_arch = "aarch64") {
            Arch::Aarch64
        } else {
            Arch::X86_64
        };
        assert_eq!(spec.arch, host);
    }

    #[test]
    fn the_form_sends_the_chosen_vm_type_arch_and_mount() {
        let (mut h, mut ends) = harness(Lang::En, two_instances());
        h.state_mut().0.instances.name = "fresh".into();
        h.run_steps(4);
        for (list, choice) in [
            ("VM type", "qemu"),
            ("Architecture", "x86_64"),
            ("Mount type", "9p"),
        ] {
            h.get_by_label(list).click();
            h.run_steps(4);
            h.get_by_label(choice).click();
            h.run_steps(4);
        }
        h.get_by_label("Create").click();
        h.run_steps(4);
        let sent = requests(&mut ends);
        let [Request::Create(spec)] = sent.as_slice() else {
            panic!("{sent:?}");
        };
        assert_eq!(
            (spec.vm_type, spec.arch, spec.mount_type),
            (VmType::Qemu, Arch::X86_64, MountType::NineP)
        );
    }

    #[test]
    fn the_choice_lists_are_labelled_in_both_languages() {
        for (lang, labels) in [
            (Lang::En, ["VM type", "Architecture", "Mount type"]),
            (Lang::Ja, ["VMの種類", "アーキテクチャ", "マウント方式"]),
        ] {
            let (h, _ends) = harness(lang, two_instances());
            for label in labels {
                h.get_by_label(label);
            }
        }
    }

    #[test]
    fn output_is_shown_once_colima_prints() {
        let (mut h, _ends) = harness(Lang::En, two_instances());
        assert!(h.query_by_label("Output").is_none());
        h.state_mut().0.apply_reply(Reply::InstanceLine {
            profile: "dev".into(),
            line: "starting vm".into(),
        });
        h.run_steps(4);
        h.get_by_label("Output");
        h.get_by_label("starting vm");
    }

    #[test]
    fn a_long_output_line_stays_on_one_line() {
        let (mut h, _ends) = harness(Lang::En, two_instances());
        for line in ["short", &format!("long{}", "x".repeat(2_000))] {
            h.state_mut().0.apply_reply(Reply::InstanceLine {
                profile: "dev".into(),
                line: line.to_string(),
            });
        }
        h.run_steps(4);
        let short = h.get_by_label("short").rect();
        let long = h.get_by_label_contains("longxxxx").rect();
        // 切り詰めた行は1行ぶん以下の高さになる。折り返すと何行ぶんにもなる。
        assert!(long.height() <= short.height() + 0.5, "{long:?} {short:?}");
    }

    #[test]
    fn no_instances_still_offers_the_create_form() {
        let mut state = two_instances();
        state.apply_snapshot(Some(Snapshot {
            updated_at: SystemTime::now(),
            instances: vec![],
            docker: BTreeMap::new(),
        }));
        let (h, _ends) = harness(Lang::En, state);
        h.get_by_label("No colima instance found");
        h.get_by_label("Create");
    }

    #[test]
    fn without_a_snapshot_nothing_is_drawn() {
        let mut state = GuiState::new();
        state.page = Page::Instances;
        let (h, _ends) = harness(Lang::En, state);
        assert!(h.query_by_label("Create").is_none());
    }
}
