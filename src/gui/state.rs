//! 画面が持つ状態。状態ファイルは読むだけで、書かない。

use super::bridge::{Cleanup, Reply, Request, image_key, network_key, volume_key};
use super::confirm::{Confirm, PendingAction};
use super::log_buffer::LogBuffer;
use super::pages::Inspect;
use super::pages::{containers, images, instances, networks, volumes};
use crate::colima::InstanceStatus;
use crate::docker::actions::PullProgress;
use crate::i18n::Msg;
use crate::snapshot::{self, DockerState, Snapshot, SnapshotError};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

/// 状態ファイルを確かめる間隔。
const POLL_EVERY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Dashboard,
    Instances,
    Containers,
    Images,
    Volumes,
    Networks,
}

impl Page {
    pub const ALL: [Page; 6] = [
        Page::Dashboard,
        Page::Instances,
        Page::Containers,
        Page::Images,
        Page::Volumes,
        Page::Networks,
    ];

    pub fn title(self) -> Msg {
        match self {
            Page::Dashboard => Msg::PageDashboard,
            Page::Instances => Msg::PageInstances,
            Page::Containers => Msg::PageContainers,
            Page::Images => Msg::PageImages,
            Page::Volumes => Msg::PageVolumes,
            Page::Networks => Msg::PageNetworks,
        }
    }

    /// 選んだインスタンスのDockerに依存する画面。
    pub fn needs_docker(self) -> bool {
        !matches!(self, Page::Dashboard | Page::Instances)
    }
}

/// 画面上部に出す通知。閉じるまで残る。
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub message: String,
    /// 失敗ではない知らせ。珊瑚色にしない。
    pub info: bool,
}

pub struct GuiState {
    pub snapshot: Option<Snapshot>,
    pub selected: Option<String>,
    pub page: Page,
    pub toasts: Vec<Toast>,
    pub confirm: Option<Confirm>,
    /// 処理を送って結果をまだ受けていないインスタンス。二重に押せないようにする。
    pub busy: HashSet<String>,
    /// 処理を送って結果をまだ受けていないコンテナのIDと、実行フォームの印。
    pub busy_ops: HashSet<String>,
    /// 最後に走らせたcolimaの出力。
    pub instance_log: LogBuffer,
    pub containers: containers::View,
    pub instances: instances::View,
    pub images: images::View,
    pub volumes: volumes::View,
    pub networks: networks::View,
    /// 開いているイメージ、ボリューム、ネットワークの詳細。
    pub inspect: Option<Inspect>,
    /// 起動時に選ぶインスタンス。`WHARFKEEP_PROFILE`。e2eが専用のインスタンスを選ぶために使う。
    pub preferred: Option<String>,
}

impl GuiState {
    pub fn new() -> Self {
        GuiState {
            snapshot: None,
            selected: None,
            page: Page::Dashboard,
            toasts: Vec::new(),
            confirm: None,
            busy: HashSet::new(),
            busy_ops: HashSet::new(),
            instance_log: LogBuffer::default(),
            containers: containers::View::default(),
            instances: instances::View::default(),
            images: images::View::default(),
            volumes: volumes::View::default(),
            networks: networks::View::default(),
            inspect: None,
            preferred: None,
        }
    }

    /// 新しい状態を取り込む。選んだインスタンスが消えていれば選び直す。
    pub fn apply_snapshot(&mut self, snapshot: Option<Snapshot>) {
        let still_there = |s: &Snapshot, name: &String| s.instances.iter().any(|i| &i.name == name);
        self.selected = match (&snapshot, self.selected.take()) {
            (Some(s), Some(name)) if still_there(s, &name) => Some(name),
            (Some(s), _) => self
                .preferred
                .clone()
                .filter(|p| still_there(s, p))
                .or_else(|| default_selection(s)),
            (None, keep) => keep,
        };
        self.snapshot = snapshot;
    }

    /// 選んだインスタンスが替わったとき、前のインスタンス向けの画面の状態を捨てる。
    /// 開いていたログの流れを止める要求を返す。
    pub fn leave_instance(&mut self) -> Option<Request> {
        self.containers.rename = None;
        self.images.tag = None;
        self.images.pull = None;
        self.images.cleanup = None;
        self.inspect = None;
        self.containers
            .detail
            .take()
            .map(|d| Request::StopLogs { id: d.id })
    }

    /// 選んだインスタンスのDockerの状態。状態ファイルに載っていなければ`None`。
    pub fn selected_docker(&self) -> Option<&DockerState> {
        let name = self.selected.as_ref()?;
        self.snapshot.as_ref()?.docker.get(name)
    }

    /// colimaの処理を始める印を付け、前の出力を捨てる。
    pub fn begin_instance_op(&mut self, profile: &str) {
        self.busy.insert(profile.to_string());
        self.instance_log.clear();
    }

    /// 確認された操作に処理中の印を付け、送る要求を返す。
    pub fn start_pending(&mut self, action: PendingAction) -> Request {
        match &action {
            PendingAction::Colima { profile, .. } => self.begin_instance_op(profile),
            PendingAction::Container { id, .. } => {
                self.busy_ops.insert(id.clone());
            }
            PendingAction::Image { id, .. } => {
                self.busy_ops.insert(image_key(id));
            }
            PendingAction::Volume { name, .. } => {
                self.busy_ops.insert(volume_key(name));
            }
            PendingAction::Network { id, .. } => {
                self.busy_ops.insert(network_key(id));
            }
            PendingAction::PruneVolumes { profile } => {
                self.busy_ops.insert(Cleanup::Volumes.key(profile));
            }
            PendingAction::PruneNetworks { profile } => {
                self.busy_ops.insert(Cleanup::Networks.key(profile));
            }
        }
        action.into_request()
    }

    /// 処理側の返事を画面の状態に映す。
    pub fn apply_reply(&mut self, reply: Reply) {
        match reply {
            Reply::ColimaDone { profile, error } => {
                self.busy.remove(&profile);
                self.toast_error(error);
            }
            Reply::InstanceLine { line, .. } => self.instance_log.push(line),
            Reply::OpDone { key, error } => {
                self.busy_ops.remove(&key);
                self.toast_error(error);
            }
            Reply::Failed(message) => self.push_toast(message),
            Reply::Notice(message) => self.push_notice(message),
            Reply::ResourceInspected { kind, target, text } => {
                if let Some(i) = self.inspect.as_mut().filter(|i| i.is(kind, &target)) {
                    i.text = Some(text);
                }
            }
            Reply::PullProgress { profile, progress } => self.pull_progress(&profile, &progress),
            Reply::Inspected { id, text } => {
                if let Some(d) = self.containers.detail_for(&id) {
                    d.inspect = Some(text);
                }
            }
            Reply::UsageSample { id, usage } => {
                if let Some(d) = self.containers.detail_for(&id) {
                    d.usage = Some(usage);
                }
            }
            Reply::LogLine { id, line } => {
                if let Some(d) = self.containers.detail_for(&id) {
                    d.log.push(line);
                }
            }
        }
    }

    /// 切り替える前のインスタンスの取得は、今の進捗に混ぜない。
    fn pull_progress(&mut self, profile: &str, progress: &PullProgress) {
        if self.selected.as_deref() == Some(profile)
            && let Some(pull) = self.images.pull.as_mut()
        {
            pull.update(progress);
        }
    }

    fn toast_error(&mut self, error: Option<String>) {
        if let Some(message) = error {
            self.push_toast(message);
        }
    }

    pub fn push_toast(&mut self, message: String) {
        self.push(message, false);
    }

    pub fn push_notice(&mut self, message: String) {
        self.push(message, true);
    }

    fn push(&mut self, message: String, info: bool) {
        if self.toasts.last().is_none_or(|t| t.message != message) {
            self.toasts.push(Toast { message, info });
        }
    }
}

impl Default for GuiState {
    fn default() -> Self {
        Self::new()
    }
}

/// 起動直後の選択。最初の実行中のインスタンス。なければ最初のインスタンス。
pub fn default_selection(s: &Snapshot) -> Option<String> {
    s.instances
        .iter()
        .find(|i| i.status == InstanceStatus::Running)
        .or(s.instances.first())
        .map(|i| i.name.clone())
}

/// 状態ファイルがない、または古いなら、デーモンが止まっているとみなす。
pub fn daemon_stopped(snapshot: Option<&Snapshot>, now: SystemTime) -> bool {
    snapshot.is_none_or(|s| snapshot::is_stale(now, s))
}

/// 状態ファイルの更新時刻を1秒ごとに見て、変わったときだけ読み直す。
pub struct StateWatcher {
    path: PathBuf,
    last_check: Option<Instant>,
    last_modified: Option<SystemTime>,
}

pub enum Polled {
    /// 前回から変わっていない。
    Unchanged,
    Loaded(Option<Snapshot>),
    Failed(SnapshotError),
}

impl StateWatcher {
    pub fn new(path: PathBuf) -> Self {
        StateWatcher {
            path,
            last_check: None,
            last_modified: None,
        }
    }

    pub fn poll(&mut self, now: Instant) -> Polled {
        if self
            .last_check
            .is_some_and(|t| now.duration_since(t) < POLL_EVERY)
        {
            return Polled::Unchanged;
        }
        self.last_check = Some(now);
        let modified = std::fs::metadata(&self.path)
            .and_then(|m| m.modified())
            .ok();
        // 初回は、ファイルがなくても「なし」を知らせる。
        if self.last_modified == modified && modified.is_some() {
            return Polled::Unchanged;
        }
        self.last_modified = modified;
        match snapshot::read(&self.path) {
            Ok(s) => Polled::Loaded(s),
            Err(e) => Polled::Failed(e),
        }
    }
}
