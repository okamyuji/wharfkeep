//! コンテナ画面。一覧、絞り込み、行ごとの操作ボタン、詳細、新規実行。

use super::{format_bytes, mono_line, name_widget, paint_bar};
use crate::docker::actions::{
    RestartPolicy, RunSpec, Usage, parse_env, parse_mount, parse_port_map,
};
use crate::docker::{DockerError, check_image_ref, check_name};
use crate::gui::bridge::{Bridge, ContainerOp, Request, run_key};
use crate::gui::confirm::{Confirm, PendingAction};
use crate::gui::log_buffer::LogBuffer;
use crate::gui::state::GuiState;
use crate::i18n::{Lang, Msg};
use crate::snapshot::ContainerRow;
use eframe::egui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    Running,
    Paused,
    Stopped,
    Failed,
}

impl StatusTone {
    pub fn color(self) -> egui::Color32 {
        match self {
            StatusTone::Running => egui::Color32::from_rgb(0x3F, 0xA6, 0x6B),
            StatusTone::Paused => egui::Color32::from_rgb(0xD9, 0xA4, 0x41),
            StatusTone::Stopped => egui::Color32::from_rgb(0x8A, 0x8F, 0x98),
            StatusTone::Failed => egui::Color32::from_rgb(0xD2, 0x66, 0x5A),
        }
    }
}

/// 失敗は状態に関係なく珊瑚色。再起動中は動いているものとして緑にする。
pub fn status_tone(row: &ContainerRow) -> StatusTone {
    if row.failed {
        return StatusTone::Failed;
    }
    match row.state.as_str() {
        "running" | "restarting" => StatusTone::Running,
        "paused" => StatusTone::Paused,
        _ => StatusTone::Stopped,
    }
}

/// 色だけに頼らないための、状態の文字。
pub fn status_text(row: &ContainerRow, lang: Lang) -> String {
    let msg = if row.failed {
        Msg::StateFailed
    } else {
        match row.state.as_str() {
            "running" => Msg::StateRunning,
            "paused" => Msg::StatePaused,
            "exited" => Msg::StateStopped,
            "created" => Msg::StateCreated,
            "restarting" => Msg::StateRestarting,
            other => return other.to_string(),
        }
    };
    msg.text(lang).to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowAction {
    Start,
    Stop,
    Pause,
    Unpause,
    Restart,
    Logs,
    Shell,
    Rename,
    Delete,
}

impl RowAction {
    fn label(self) -> Msg {
        match self {
            RowAction::Start => Msg::RowStart,
            RowAction::Stop => Msg::RowStop,
            RowAction::Pause => Msg::RowPause,
            RowAction::Unpause => Msg::RowResume,
            RowAction::Restart => Msg::RowRestart,
            RowAction::Logs => Msg::RowLogs,
            RowAction::Shell => Msg::RowShell,
            RowAction::Rename => Msg::RowRename,
            RowAction::Delete => Msg::Delete,
        }
    }

    fn icon(self) -> &'static str {
        match self {
            RowAction::Start | RowAction::Unpause => "▶",
            RowAction::Stop => "⏹",
            RowAction::Pause => "⏸",
            RowAction::Restart => "⟲",
            RowAction::Logs => "≡",
            RowAction::Shell => ">_",
            RowAction::Rename => "✏",
            RowAction::Delete => "🗑",
        }
    }

    fn op(self) -> Option<ContainerOp> {
        match self {
            RowAction::Start => Some(ContainerOp::Start),
            RowAction::Stop => Some(ContainerOp::Stop),
            RowAction::Pause => Some(ContainerOp::Pause),
            RowAction::Unpause => Some(ContainerOp::Unpause),
            RowAction::Restart => Some(ContainerOp::Restart),
            RowAction::Logs | RowAction::Shell | RowAction::Rename | RowAction::Delete => None,
        }
    }
}

/// 状態ごとに押せる操作。ログ、シェル、名前変更、削除はどの状態でも出す。
pub fn row_actions(state: &str) -> Vec<RowAction> {
    use RowAction::*;
    match state {
        "running" => vec![Stop, Pause, Restart, Logs, Shell, Rename, Delete],
        "paused" => vec![Unpause, Logs, Shell, Rename, Delete],
        "restarting" => vec![Stop, Logs, Shell, Rename, Delete],
        _ => vec![Start, Logs, Shell, Rename, Delete],
    }
}

/// 止まっていないコンテナは、止めてから消す。
fn needs_force(state: &str) -> bool {
    !matches!(state, "exited" | "created")
}

/// 名前に`query`を含む行だけ。大文字と小文字は区別しない。
pub fn filter_rows<'a>(rows: &'a [ContainerRow], query: &str) -> Vec<&'a ContainerRow> {
    let q = query.to_lowercase();
    rows.iter()
        .filter(|r| r.name.to_lowercase().contains(&q))
        .collect()
}

#[derive(Debug, Default)]
pub struct RunForm {
    pub image: String,
    pub name: String,
    pub ports: String,
    /// 1行に`KEY=VALUE`を1つ。
    pub env: String,
    /// 1行に`SOURCE:/path[:ro]`を1つ。
    pub mounts: String,
    pub restart: RestartPolicy,
    /// 空白で区切った引数。空ならイメージの既定のコマンド。
    // shortcut: 引用符でまとめた引数は分けずに空白で切る。空白を含む引数が要るときは`sh -c`の文字列を1つにする形を足す。
    pub command: String,
}

/// 空の行を除いた各行。
fn lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(str::trim).filter(|l| !l.is_empty())
}

impl RunForm {
    /// 空の入力は、まだ何も言わない。ポートはコンマか空白で区切る。
    pub fn spec(&self) -> Result<RunSpec, DockerError> {
        let image = self.image.trim();
        check_image_ref(image)?;
        let name = self.name.trim();
        if !name.is_empty() {
            check_name(name)?;
        }
        let ports = self
            .ports
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|p| !p.is_empty())
            .map(parse_port_map)
            .collect::<Result<_, _>>()?;
        let env = lines(&self.env).map(parse_env).collect::<Result<_, _>>()?;
        let mounts = lines(&self.mounts)
            .map(parse_mount)
            .collect::<Result<_, _>>()?;
        let cmd: Vec<String> = self.command.split_whitespace().map(String::from).collect();
        Ok(RunSpec {
            image: image.to_string(),
            name: (!name.is_empty()).then(|| name.to_string()),
            ports,
            env,
            mounts,
            restart: self.restart,
            cmd: (!cmd.is_empty()).then_some(cmd),
        })
    }
}

#[derive(Debug)]
pub struct Detail {
    pub id: String,
    pub name: String,
    pub follow: bool,
    pub log: LogBuffer,
    pub inspect: Option<String>,
    pub usage: Option<Usage>,
}

/// 名前変更の入力。
#[derive(Debug)]
pub struct RenameForm {
    pub id: String,
    pub name: String,
    pub new_name: String,
}

/// Logsを押したときの、追従しない1回きりの取得。行が来たかと、流れが終わったかを覚える。
/// 追従する流れは終わらないので、ログが無いことは追従しない取得の終わりでしか分からない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogProbe {
    pub id: String,
    pub name: String,
    pub seen: bool,
    pub ended: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// 1行でもあれば、今までどおり下の欄を開く。
    Open,
    /// 1行も無ければ、欄を開かずにダイアログで伝える。
    NoLogs,
}

/// 流れが終わった取得を外し、どうするかを決める。終わっていなければ何もしない。
pub fn settle(probe: &mut Option<LogProbe>) -> Option<(LogProbe, ProbeOutcome)> {
    let p = probe.take_if(|p| p.ended)?;
    let outcome = if p.seen {
        ProbeOutcome::Open
    } else {
        ProbeOutcome::NoLogs
    };
    Some((p, outcome))
}

#[derive(Debug, Default)]
pub struct View {
    pub filter: String,
    pub detail: Option<Detail>,
    pub run: RunForm,
    pub rename: Option<RenameForm>,
    pub probe: Option<LogProbe>,
    /// ログが無いと伝えるダイアログのコンテナ名。
    pub no_logs: Option<String>,
}

impl View {
    /// 開いている詳細が`id`のものであれば返す。閉じたあとに届いた返事は捨てる。
    pub fn detail_for(&mut self, id: &str) -> Option<&mut Detail> {
        self.detail.as_mut().filter(|d| d.id == id)
    }

    /// 読んでいる最中の取得が`id`のものであれば返す。
    pub fn probe_for(&mut self, id: &str) -> Option<&mut LogProbe> {
        self.probe.as_mut().filter(|p| p.id == id)
    }

    /// 届いたログの1行。詳細が開いていれば足し、取得の最中なら行があったと覚える。
    pub fn log_line(&mut self, id: &str, line: String) {
        if let Some(d) = self.detail_for(id) {
            d.log.push(line);
        }
        if let Some(p) = self.probe_for(id) {
            p.seen = true;
        }
    }

    /// ログの流れが終わった。取得の最中なら、読み終えたと覚える。
    pub fn logs_ended(&mut self, id: &str) {
        if let Some(p) = self.probe_for(id) {
            p.ended = true;
        }
    }
}

/// 詳細を開く前に、今あるログを追従せずに1回読む。
fn start_probe(state: &mut GuiState, bridge: &Bridge, profile: &str, row: &ContainerRow) {
    close_detail(state, bridge);
    state.containers.probe = Some(LogProbe {
        id: row.id.clone(),
        name: row.name.clone(),
        seen: false,
        ended: false,
    });
    bridge.send(Request::Logs {
        profile: profile.to_string(),
        id: row.id.clone(),
        follow: false,
    });
}

/// 読み終えた取得に応じて、詳細を開くか、ログが無いと伝える。
fn settle_probe(state: &mut GuiState, bridge: &Bridge, profile: &str) {
    match settle(&mut state.containers.probe) {
        Some((p, ProbeOutcome::Open)) => open_detail(state, bridge, profile, &p.id, &p.name),
        Some((p, ProbeOutcome::NoLogs)) => state.containers.no_logs = Some(p.name),
        None => {}
    }
}

fn no_logs_dialog(ui: &mut egui::Ui, state: &mut GuiState, lang: Lang) {
    let Some(name) = &state.containers.no_logs else {
        return;
    };
    let message = Msg::NoLogs.text(lang).replace("{name}", name);
    let response = egui::Modal::new(egui::Id::unique("wharfkeep-no-logs")).show(ui.ctx(), |ui| {
        ui.label(message);
        ui.button(Msg::Close.text(lang)).clicked()
    });
    if response.inner || response.should_close() {
        state.containers.no_logs = None;
    }
}

/// 詳細を開く。ログの流れ、inspect、その時点のCPUとメモリを頼む。
fn open_detail(state: &mut GuiState, bridge: &Bridge, profile: &str, id: &str, name: &str) {
    close_detail(state, bridge);
    state.containers.detail = Some(Detail {
        id: id.to_string(),
        name: name.to_string(),
        follow: true,
        log: LogBuffer::default(),
        inspect: None,
        usage: None,
    });
    let (profile, id) = (profile.to_string(), id.to_string());
    bridge.send(Request::Logs {
        profile: profile.clone(),
        id: id.clone(),
        follow: true,
    });
    bridge.send(Request::Inspect {
        profile: profile.clone(),
        id: id.clone(),
    });
    bridge.send(Request::Usage { profile, id });
}

fn close_detail(state: &mut GuiState, bridge: &Bridge) {
    if let Some(d) = state.containers.detail.take() {
        bridge.send(Request::StopLogs { id: d.id });
    }
}

/// 行のボタンが押されたときの動き。操作は1回の押下で要求になる。
fn perform(
    state: &mut GuiState,
    bridge: &Bridge,
    lang: Lang,
    profile: &str,
    row: &ContainerRow,
    action: RowAction,
) {
    if let Some(op) = action.op() {
        state.busy_ops.insert(row.id.clone());
        bridge.send(Request::Container {
            profile: profile.to_string(),
            id: row.id.clone(),
            op,
        });
        return;
    }
    match action {
        RowAction::Logs => start_probe(state, bridge, profile, row),
        RowAction::Rename => {
            state.containers.rename = Some(RenameForm {
                id: row.id.clone(),
                name: row.name.clone(),
                new_name: row.name.clone(),
            });
        }
        RowAction::Shell => bridge.send(Request::Shell {
            profile: profile.to_string(),
            container: Some(row.id.clone()),
        }),
        _ => {
            let force = needs_force(&row.state);
            let msg = if force {
                Msg::ConfirmForceDeleteContainer
            } else {
                Msg::ConfirmDeleteContainer
            };
            state.confirm = Some(Confirm {
                message: msg.text(lang).replace("{name}", &row.name),
                action: PendingAction::Container {
                    profile: profile.to_string(),
                    id: row.id.clone(),
                    force,
                },
            });
        }
    }
}

pub fn show(
    ui: &mut egui::Ui,
    state: &mut GuiState,
    bridge: &Bridge,
    lang: Lang,
    profile: &str,
    rows: &[ContainerRow],
) {
    settle_probe(state, bridge, profile);
    no_logs_dialog(ui, state, lang);
    run_form(ui, state, bridge, lang, profile);
    rename_form(ui, state, bridge, lang, profile);
    ui.add(
        egui::TextEdit::singleline(&mut state.containers.filter)
            .hint_text(Msg::FilterHint.text(lang)),
    );
    if state.containers.detail.is_some() {
        egui::Panel::bottom("container-detail")
            .resizable(true)
            .show(ui, |ui| detail(ui, state, bridge, lang, profile));
    }
    let shown = filter_rows(rows, &state.containers.filter);
    if shown.is_empty() {
        ui.label(Msg::NoContainers.text(lang));
        return;
    }
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("container-rows")
            .striped(true)
            .num_columns(5)
            .show(ui, |ui| {
                for row in shown {
                    row_ui(ui, state, bridge, lang, profile, row);
                    ui.end_row();
                }
            });
    });
}

/// 表の1行。ボタンは左から右へ並べる。支援技術が読む順をボタンの並びと同じにするため。
fn row_ui(
    ui: &mut egui::Ui,
    state: &mut GuiState,
    bridge: &Bridge,
    lang: Lang,
    profile: &str,
    row: &ContainerRow,
) {
    let busy = state.busy_ops.contains(&row.id);
    let tone = status_tone(row);
    let mut clicked = None;
    ui.horizontal(|ui| {
        paint_bar(ui, tone);
        ui.label(egui::RichText::new(status_text(row, lang)).color(tone.color()));
    });
    ui.horizontal(|ui| {
        ui.strong(&row.name);
        if busy {
            ui.spinner();
        }
    });
    ui.horizontal(|ui| {
        for action in row_actions(&row.state) {
            let name = format!("{} {}", action.label().text(lang), row.name);
            let response = ui
                .add_enabled(!busy, egui::Button::new(action.icon()))
                .on_hover_text(&name)
                .on_disabled_hover_text(&name);
            name_widget(ui, &response, &name);
            if response.clicked() {
                clicked = Some(action);
            }
        }
    });
    ui.weak(&row.image);
    ui.weak(&row.ports);
    if let Some(action) = clicked {
        perform(state, bridge, lang, profile, row, action);
    }
}

fn rename_form(
    ui: &mut egui::Ui,
    state: &mut GuiState,
    bridge: &Bridge,
    lang: Lang,
    profile: &str,
) {
    let Some(form) = state.containers.rename.as_mut() else {
        return;
    };
    let new_name = form.new_name.trim().to_string();
    let check = check_name(&new_name);
    let (mut apply, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        ui.strong(format!("{} {}", Msg::RowRename.text(lang), form.name));
        let caption = ui.label(Msg::FieldNewName.text(lang));
        ui.add(egui::TextEdit::singleline(&mut form.new_name).desired_width(200.0))
            .labelled_by(caption.id);
        apply = ui
            .add_enabled(
                check.is_ok(),
                egui::Button::new(Msg::ApplyRename.text(lang)),
            )
            .clicked();
        cancel = ui.button(Msg::Cancel.text(lang)).clicked();
    });
    if let Err(e) = &check {
        ui.colored_label(crate::gui::WARN, e.message(lang));
    }
    if apply {
        state.busy_ops.insert(form.id.clone());
        bridge.send(Request::Rename {
            profile: profile.to_string(),
            id: form.id.clone(),
            name: new_name,
        });
    }
    if apply || cancel {
        state.containers.rename = None;
    }
}

fn run_form(ui: &mut egui::Ui, state: &mut GuiState, bridge: &Bridge, lang: Lang, profile: &str) {
    egui::CollapsingHeader::new(Msg::RunTitle.text(lang)).show(ui, |ui| {
        let form = &mut state.containers.run;
        for (label, field) in [
            (Msg::FieldImage, &mut form.image),
            (Msg::FieldName, &mut form.name),
            (Msg::FieldPorts, &mut form.ports),
            (Msg::FieldCommand, &mut form.command),
        ] {
            ui.horizontal(|ui| {
                let caption = ui.label(label.text(lang));
                ui.add(egui::TextEdit::singleline(field).desired_width(260.0))
                    .labelled_by(caption.id);
            });
        }
        for (label, field) in [
            (Msg::FieldEnv, &mut form.env),
            (Msg::FieldMounts, &mut form.mounts),
        ] {
            ui.horizontal(|ui| {
                let caption = ui.label(label.text(lang));
                ui.add(
                    egui::TextEdit::multiline(field)
                        .desired_rows(2)
                        .desired_width(260.0),
                )
                .labelled_by(caption.id);
            });
        }
        super::choice(
            ui,
            Msg::FieldRestart.text(lang),
            &mut form.restart,
            RestartPolicy::ALL,
            RestartPolicy::as_str,
        );
        let spec = state.containers.run.spec();
        if let Err(e) = &spec
            && !state.containers.run.image.trim().is_empty()
        {
            ui.colored_label(crate::gui::WARN, e.message(lang));
        }
        let key = run_key(profile);
        let idle = !state.busy_ops.contains(&key);
        let run = ui.add_enabled(idle && spec.is_ok(), egui::Button::new(Msg::Run.text(lang)));
        if run.clicked()
            && let Ok(spec) = spec
        {
            state.busy_ops.insert(key);
            bridge.send(Request::Run {
                profile: profile.to_string(),
                spec,
            });
        }
    });
}

fn detail(ui: &mut egui::Ui, state: &mut GuiState, bridge: &Bridge, lang: Lang, profile: &str) {
    let Some(d) = state.containers.detail.as_mut() else {
        return;
    };
    let mut close = false;
    let (id, mut refresh_logs) = (d.id.clone(), false);
    ui.horizontal(|ui| {
        ui.strong(&d.name);
        if ui.checkbox(&mut d.follow, Msg::Follow.text(lang)).changed() {
            refresh_logs = true;
        }
        if ui.button(Msg::RefreshUsage.text(lang)).clicked() {
            bridge.send(Request::Usage {
                profile: profile.to_string(),
                id: id.clone(),
            });
        }
        close = ui.button(Msg::Close.text(lang)).clicked();
    });
    if let Some(u) = &d.usage {
        ui.label(
            Msg::UsageLine
                .text(lang)
                .replace("{cpu}", &format!("{:.1}", u.cpu_percent))
                .replace("{mem}", &format_bytes(u.mem_bytes))
                .replace("{limit}", &format_bytes(u.mem_limit)),
        );
    }
    if let Some(text) = &d.inspect {
        egui::CollapsingHeader::new(Msg::InspectTitle.text(lang)).show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("inspect")
                .max_height(160.0)
                .show(ui, |ui| ui.monospace(text));
        });
    }
    let line_height = ui.text_style_height(&egui::TextStyle::Monospace);
    egui::ScrollArea::vertical()
        .id_salt("logs")
        .stick_to_bottom(d.follow)
        .show_rows(ui, line_height, d.log.len(), |ui, range| {
            for line in d.log.lines().skip(range.start).take(range.len()) {
                mono_line(ui, line);
            }
        });
    if refresh_logs {
        // 追従を切り替えると、流れを頭から読み直す。
        d.log.clear();
        bridge.send(Request::Logs {
            profile: profile.to_string(),
            id: id.clone(),
            follow: d.follow,
        });
    }
    if close {
        close_detail(state, bridge);
    }
}

#[cfg(test)]
mod tests;
