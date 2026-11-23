/// 画面に出す言語。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Ja,
}

impl Lang {
    /// `WHARFKEEP_LANG`（`en`か`ja`）があればそれを、なければOSの言語を使う。
    pub fn detect() -> Lang {
        let env = std::env::var("WHARFKEEP_LANG").ok();
        let lang = Lang::resolve(env.as_deref(), sys_locale::get_locale().as_deref());
        if let Some(v) = env.as_deref()
            && !v.is_empty()
            && Lang::parse_override(v).is_none()
        {
            eprintln!(
                "wharfkeep: WHARFKEEP_LANG={v}: {}",
                Msg::LangEnvInvalid.text(lang)
            );
        }
        lang
    }

    /// フォントの選択に使う言語タグ。同じ漢字でも日本語と中国語で字形が違うため。
    pub fn tag(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ja => "ja",
        }
    }

    fn resolve(env: Option<&str>, locale: Option<&str>) -> Lang {
        env.and_then(Lang::parse_override)
            .unwrap_or_else(|| locale.map_or(Lang::En, Lang::from_locale))
    }

    fn parse_override(v: &str) -> Option<Lang> {
        if v.eq_ignore_ascii_case("en") {
            Some(Lang::En)
        } else if v.eq_ignore_ascii_case("ja") {
            Some(Lang::Ja)
        } else {
            None
        }
    }

    /// BCP 47（`ja-JP`）とPOSIX（`ja_JP`）の両方の形を受け、言語の部分だけを比べる。
    fn from_locale(s: &str) -> Lang {
        let lang = s.split(['-', '_']).next().unwrap_or("");
        if lang.eq_ignore_ascii_case("ja") {
            Lang::Ja
        } else {
            Lang::En
        }
    }
}

/// 画面に出す文言。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Msg {
    OpenWindow,
    Quit,
    UnknownSubcommand,
    NoHomeDir,
    LockFailed,
    RuntimeFailed,
    OpenWindowFailed,
    EventLoopCreateFailed,
    EventLoopStopped,
    LangEnvInvalid,
    ColimaParse,
    ColimaSocket,
    ColimaProfile,
    ColimaCpus,
    ColimaMemory,
    ColimaDisk,
    ColimaSpawn,
    ColimaFailed,
    ColimaOutputTooLarge,
    ColimaTimeout,
    SnapshotIo,
    SnapshotParse,
    SnapshotTooLarge,
    DockerConnect,
    DockerApi,
    DockerPort,
    DockerInvalidId,
    StartInstance,
    StopInstance,
    StartAtLogin,
    IconError,
    IconNormal,
    IconIdle,
    InstanceNotRunning,
    DockerEventsEnded,
    TrayActionFailed,
    AutostartFailed,
    SettingsSaveFailed,
    SnapshotWriteFailed,
    TempCleanupFailed,
    DaemonNotRunning,
    StartDaemon,
    DaemonStartFailed,
    StartInstanceButton,
    DockerUnavailable,
    Cancel,
    Delete,
    Dismiss,
    RaiseFailed,
    InstanceLabel,
    NoInstances,
    InstanceExists,
    PageDashboard,
    PageInstances,
    PageContainers,
    PageImages,
    PageVolumes,
    PageNetworks,
    StateRunning,
    StatePaused,
    StateStopped,
    StateCreated,
    StateRestarting,
    StateFailed,
    RowStart,
    RowStop,
    RowPause,
    RowResume,
    RowRestart,
    RowLogs,
    RowShell,
    DashInstances,
    DashResources,
    DashContainers,
    DashImages,
    DashVolumes,
    DashNetworks,
    DiskUsageTitle,
    DiskImages,
    DiskContainers,
    DiskVolumes,
    DiskBuildCache,
    NewInstance,
    FieldName,
    FieldCpus,
    FieldMemory,
    FieldDisk,
    Create,
    OutputTitle,
    ConfirmDeleteInstance,
    FilterHint,
    NoContainers,
    ConfirmDeleteContainer,
    ConfirmForceDeleteContainer,
    Follow,
    Close,
    InspectTitle,
    UsageLine,
    RefreshUsage,
    RunTitle,
    FieldImage,
    FieldPorts,
    Run,
    TerminalFailed,
    PullTitle,
    Pull,
    NoImages,
    NoVolumes,
    NoNetworks,
    Cleanup,
    CleanupImagesTitle,
    CleanupImagesNone,
    CleanupImagesDangling,
    CleanupImagesUnused,
    CleanupBuildCache,
    Unknown,
    ForceDeleteImage,
    ConfirmDeleteImage,
    ConfirmDeleteVolume,
    ConfirmDeleteNetwork,
    ConfirmPruneVolumes,
    ConfirmPruneNetworks,
    Freed,
    NetworksRemoved,
    NewVolume,
    NewNetwork,
    FieldDriver,
    BuiltinNetwork,
    RowTag,
    RowInspect,
    ApplyTag,
    FieldRepo,
    FieldTagName,
    DashMoreInstances,
    FieldEnv,
    FieldMounts,
    FieldRestart,
    FieldCommand,
    DockerEnv,
    DockerMount,
    DockerVolumeNameTooLong,
    RowRename,
    FieldNewName,
    ApplyRename,
    FieldVmType,
    FieldArch,
    FieldMountType,
    ColimaSucceeded,
    Untagged,
    DeletedItem,
    ContainerDeleted,
}

impl Msg {
    #[cfg(test)]
    const ALL: [Msg; 151] = [
        Msg::OpenWindow,
        Msg::Quit,
        Msg::UnknownSubcommand,
        Msg::NoHomeDir,
        Msg::LockFailed,
        Msg::RuntimeFailed,
        Msg::OpenWindowFailed,
        Msg::EventLoopCreateFailed,
        Msg::EventLoopStopped,
        Msg::LangEnvInvalid,
        Msg::ColimaParse,
        Msg::ColimaSocket,
        Msg::ColimaProfile,
        Msg::ColimaCpus,
        Msg::ColimaMemory,
        Msg::ColimaDisk,
        Msg::ColimaSpawn,
        Msg::ColimaFailed,
        Msg::ColimaOutputTooLarge,
        Msg::ColimaTimeout,
        Msg::SnapshotIo,
        Msg::SnapshotParse,
        Msg::SnapshotTooLarge,
        Msg::DockerConnect,
        Msg::DockerApi,
        Msg::DockerPort,
        Msg::DockerInvalidId,
        Msg::StartInstance,
        Msg::StopInstance,
        Msg::StartAtLogin,
        Msg::IconError,
        Msg::IconNormal,
        Msg::IconIdle,
        Msg::InstanceNotRunning,
        Msg::DockerEventsEnded,
        Msg::TrayActionFailed,
        Msg::AutostartFailed,
        Msg::SettingsSaveFailed,
        Msg::SnapshotWriteFailed,
        Msg::TempCleanupFailed,
        Msg::DaemonNotRunning,
        Msg::StartDaemon,
        Msg::DaemonStartFailed,
        Msg::StartInstanceButton,
        Msg::DockerUnavailable,
        Msg::Cancel,
        Msg::Delete,
        Msg::Dismiss,
        Msg::RaiseFailed,
        Msg::InstanceLabel,
        Msg::NoInstances,
        Msg::InstanceExists,
        Msg::PageDashboard,
        Msg::PageInstances,
        Msg::PageContainers,
        Msg::PageImages,
        Msg::PageVolumes,
        Msg::PageNetworks,
        Msg::StateRunning,
        Msg::StatePaused,
        Msg::StateStopped,
        Msg::StateCreated,
        Msg::StateRestarting,
        Msg::StateFailed,
        Msg::RowStart,
        Msg::RowStop,
        Msg::RowPause,
        Msg::RowResume,
        Msg::RowRestart,
        Msg::RowLogs,
        Msg::RowShell,
        Msg::DashInstances,
        Msg::DashResources,
        Msg::DashContainers,
        Msg::DashImages,
        Msg::DashVolumes,
        Msg::DashNetworks,
        Msg::DiskUsageTitle,
        Msg::DiskImages,
        Msg::DiskContainers,
        Msg::DiskVolumes,
        Msg::DiskBuildCache,
        Msg::NewInstance,
        Msg::FieldName,
        Msg::FieldCpus,
        Msg::FieldMemory,
        Msg::FieldDisk,
        Msg::Create,
        Msg::OutputTitle,
        Msg::ConfirmDeleteInstance,
        Msg::FilterHint,
        Msg::NoContainers,
        Msg::ConfirmDeleteContainer,
        Msg::ConfirmForceDeleteContainer,
        Msg::Follow,
        Msg::Close,
        Msg::InspectTitle,
        Msg::UsageLine,
        Msg::RefreshUsage,
        Msg::RunTitle,
        Msg::FieldImage,
        Msg::FieldPorts,
        Msg::Run,
        Msg::TerminalFailed,
        Msg::PullTitle,
        Msg::Pull,
        Msg::NoImages,
        Msg::NoVolumes,
        Msg::NoNetworks,
        Msg::Cleanup,
        Msg::CleanupImagesTitle,
        Msg::CleanupImagesNone,
        Msg::CleanupImagesDangling,
        Msg::CleanupImagesUnused,
        Msg::CleanupBuildCache,
        Msg::Unknown,
        Msg::ForceDeleteImage,
        Msg::ConfirmDeleteImage,
        Msg::ConfirmDeleteVolume,
        Msg::ConfirmDeleteNetwork,
        Msg::ConfirmPruneVolumes,
        Msg::ConfirmPruneNetworks,
        Msg::Freed,
        Msg::NetworksRemoved,
        Msg::NewVolume,
        Msg::NewNetwork,
        Msg::FieldDriver,
        Msg::BuiltinNetwork,
        Msg::RowTag,
        Msg::RowInspect,
        Msg::ApplyTag,
        Msg::FieldRepo,
        Msg::FieldTagName,
        Msg::DashMoreInstances,
        Msg::FieldEnv,
        Msg::FieldMounts,
        Msg::FieldRestart,
        Msg::FieldCommand,
        Msg::DockerEnv,
        Msg::DockerMount,
        Msg::DockerVolumeNameTooLong,
        Msg::RowRename,
        Msg::FieldNewName,
        Msg::ApplyRename,
        Msg::FieldVmType,
        Msg::FieldArch,
        Msg::FieldMountType,
        Msg::ColimaSucceeded,
        Msg::Untagged,
        Msg::DeletedItem,
        Msg::ContainerDeleted,
    ];

    pub fn text(self, lang: Lang) -> &'static str {
        let (_, en, ja) = TEXTS
            .iter()
            .find(|(m, _, _)| *m == self)
            .expect("every Msg has a row in TEXTS");
        match lang {
            Lang::En => en,
            Lang::Ja => ja,
        }
    }
}

/// 文言の表。English、日本語の順。
const TEXTS: &[(Msg, &str, &str)] = &[
    (Msg::OpenWindow, "Open Window", "ウィンドウを開く"),
    (Msg::Quit, "Quit", "終了"),
    (
        Msg::UnknownSubcommand,
        "unknown subcommand (use daemon or gui)",
        "不明なサブコマンドです（daemon か gui を指定してください）",
    ),
    (
        Msg::NoHomeDir,
        "cannot find the home directory, so the data directory is unknown",
        "ホームディレクトリが分からないため、データの置き場所を決められません",
    ),
    (
        Msg::LockFailed,
        "cannot take the lock",
        "ロックを取れません",
    ),
    (
        Msg::RuntimeFailed,
        "cannot start the monitoring thread",
        "監視のスレッドを始められません",
    ),
    (
        Msg::OpenWindowFailed,
        "cannot open the window",
        "ウィンドウを開けません",
    ),
    (
        Msg::EventLoopCreateFailed,
        "cannot create the event loop",
        "イベントループを作れません",
    ),
    (
        Msg::EventLoopStopped,
        "the event loop stopped",
        "イベントループが止まりました",
    ),
    (
        Msg::LangEnvInvalid,
        "unsupported value (use en or ja); using the OS language",
        "使えない値です（en か ja）。OSの言語を使います",
    ),
    (
        Msg::ColimaParse,
        "cannot read the colima output at line",
        "colima の出力を読めません（行）",
    ),
    (
        Msg::ColimaSocket,
        "unexpected docker socket address",
        "docker のソケットのアドレスが想定外です",
    ),
    (
        Msg::ColimaProfile,
        "invalid profile name (use A-Z a-z 0-9 . _ - only, not starting with -)",
        "プロフィール名が不正です（使える文字は A-Z a-z 0-9 . _ - だけ。先頭に - は使えません）",
    ),
    (
        Msg::ColimaCpus,
        "CPU count must be 1 or more",
        "CPU 数は 1 以上にしてください",
    ),
    (
        Msg::ColimaMemory,
        "memory must be a finite number above 0 (GiB)",
        "メモリは 0 より大きい有限の数（GiB）にしてください",
    ),
    (
        Msg::ColimaDisk,
        "disk size must be 1 GiB or more",
        "ディスクは 1 GiB 以上にしてください",
    ),
    (
        Msg::ColimaSpawn,
        "cannot run colima",
        "colima を実行できません",
    ),
    (
        Msg::ColimaFailed,
        "colima failed (exit code)",
        "colima が失敗しました（終了コード）",
    ),
    (
        Msg::ColimaOutputTooLarge,
        "colima printed too much output (over 1 MiB)",
        "colima の出力が大きすぎます（1 MiB 超）",
    ),
    (
        Msg::ColimaTimeout,
        "colima did not answer within 10 seconds",
        "colima が 10 秒以内に応答しません",
    ),
    (
        Msg::SnapshotIo,
        "cannot read the state file",
        "状態ファイルを読めません",
    ),
    (
        Msg::SnapshotParse,
        "the state file is not valid",
        "状態ファイルの形が不正です",
    ),
    (
        Msg::SnapshotTooLarge,
        "the state file is too large (over 16 MiB)",
        "状態ファイルが大きすぎます（16 MiB 超）",
    ),
    (
        Msg::DockerConnect,
        "cannot connect to the docker socket",
        "docker のソケットに接続できません",
    ),
    (
        Msg::DockerApi,
        "docker returned an error",
        "docker がエラーを返しました",
    ),
    (
        Msg::DockerPort,
        "invalid port mapping (use HOST:CONTAINER or HOST:CONTAINER/udp, ports 1-65535)",
        "ポートの対応が不正です（HOST:CONTAINER か HOST:CONTAINER/udp の形で、ポートは 1〜65535）",
    ),
    (
        Msg::DockerInvalidId,
        "invalid name, ID or image reference (container, volume and network names: A-Z a-z 0-9 . _ - only, 2 characters or more, not starting with . or -)",
        "名前、ID、イメージ参照が不正です（コンテナ、ボリューム、ネットワークの名前は A-Z a-z 0-9 . _ - だけ。2 文字以上で、先頭に . と - は使えません）",
    ),
    (Msg::StartInstance, "Start {name}", "{name} を起動"),
    (Msg::StopInstance, "Stop {name}", "{name} を停止"),
    (Msg::StartAtLogin, "Start at Login", "ログイン時に起動"),
    (
        Msg::IconError,
        "wharfkeep: a container failed",
        "wharfkeep: 失敗したコンテナがあります",
    ),
    (Msg::IconNormal, "wharfkeep: running", "wharfkeep: 実行中"),
    (
        Msg::IconIdle,
        "wharfkeep: no instance running",
        "wharfkeep: 実行中のインスタンスはありません",
    ),
    (
        Msg::InstanceNotRunning,
        "the instance is not running",
        "インスタンスが動いていません",
    ),
    (
        Msg::DockerEventsEnded,
        "the docker event stream ended",
        "docker のイベントの購読が切れました",
    ),
    (
        Msg::TrayActionFailed,
        "the menu action failed",
        "メニューの操作が失敗しました",
    ),
    (
        Msg::AutostartFailed,
        "cannot update the login item",
        "ログイン時の起動を設定できません",
    ),
    (
        Msg::SettingsSaveFailed,
        "cannot save the settings",
        "設定を保存できません",
    ),
    (
        Msg::SnapshotWriteFailed,
        "cannot write the state file",
        "状態ファイルを書けません",
    ),
    (
        Msg::TempCleanupFailed,
        "cannot remove leftover temporary files",
        "残った一時ファイルを消せません",
    ),
    (
        Msg::DaemonNotRunning,
        "The daemon is not running",
        "デーモンが動いていません",
    ),
    (Msg::StartDaemon, "Start daemon", "デーモンを起動"),
    (
        Msg::DaemonStartFailed,
        "cannot start the daemon",
        "デーモンを起動できません",
    ),
    (
        Msg::StartInstanceButton,
        "Start instance",
        "インスタンスを起動",
    ),
    (
        Msg::DockerUnavailable,
        "Docker is not available on this instance",
        "このインスタンスでは Docker を使えません",
    ),
    (Msg::Cancel, "Cancel", "キャンセル"),
    (Msg::Delete, "Delete", "削除"),
    (Msg::Dismiss, "Dismiss", "閉じる"),
    (
        Msg::RaiseFailed,
        "cannot bring the open window to the front",
        "開いているウィンドウを前面に出せません",
    ),
    (Msg::InstanceLabel, "Instance", "インスタンス"),
    (
        Msg::NoInstances,
        "No colima instance found",
        "colima のインスタンスがありません",
    ),
    (
        Msg::InstanceExists,
        "An instance with this name already exists",
        "同じ名前のインスタンスがあります",
    ),
    (Msg::PageDashboard, "Dashboard", "ダッシュボード"),
    (Msg::PageInstances, "Instances", "インスタンス"),
    (Msg::PageContainers, "Containers", "コンテナ"),
    (Msg::PageImages, "Images", "イメージ"),
    (Msg::PageVolumes, "Volumes", "ボリューム"),
    (Msg::PageNetworks, "Networks", "ネットワーク"),
    (Msg::StateRunning, "Running", "実行中"),
    (Msg::StatePaused, "Paused", "一時停止中"),
    (Msg::StateStopped, "Stopped", "停止中"),
    (Msg::StateCreated, "Created", "作成済み"),
    (Msg::StateRestarting, "Restarting", "再起動中"),
    (Msg::StateFailed, "Failed", "失敗"),
    (Msg::RowStart, "Start", "開始"),
    (Msg::RowStop, "Stop", "停止"),
    (Msg::RowPause, "Pause", "一時停止"),
    (Msg::RowResume, "Resume", "再開"),
    (Msg::RowRestart, "Restart", "再起動"),
    (Msg::RowLogs, "Logs", "ログ"),
    (Msg::RowShell, "Shell", "シェル"),
    (
        Msg::DashInstances,
        "Instances: {n} ({r} running)",
        "インスタンス: {n}（実行中 {r}）",
    ),
    (
        Msg::DashResources,
        "CPU {cpu} / memory {mem}",
        "CPU {cpu} / メモリ {mem}",
    ),
    (
        Msg::DashContainers,
        "Containers: {n} ({r} running)",
        "コンテナ: {n}（実行中 {r}）",
    ),
    (Msg::DashImages, "Images: {n}", "イメージ: {n}"),
    (Msg::DashVolumes, "Volumes: {n}", "ボリューム: {n}"),
    (Msg::DashNetworks, "Networks: {n}", "ネットワーク: {n}"),
    (Msg::DiskUsageTitle, "Disk usage", "ディスク使用量"),
    (Msg::DiskImages, "Images {size}", "イメージ {size}"),
    (Msg::DiskContainers, "Containers {size}", "コンテナ {size}"),
    (Msg::DiskVolumes, "Volumes {size}", "ボリューム {size}"),
    (
        Msg::DiskBuildCache,
        "Build cache {size}",
        "ビルドキャッシュ {size}",
    ),
    (Msg::NewInstance, "New instance", "新しいインスタンス"),
    (Msg::FieldName, "Name", "名前"),
    (Msg::FieldCpus, "CPUs", "CPU 数"),
    (Msg::FieldMemory, "Memory (GiB)", "メモリ（GiB）"),
    (Msg::FieldDisk, "Disk (GiB)", "ディスク（GiB）"),
    (Msg::Create, "Create", "作成"),
    (Msg::OutputTitle, "Output", "出力"),
    (
        Msg::ConfirmDeleteInstance,
        "Delete instance {name}? Its containers, images and volumes are lost.",
        "インスタンス {name} を削除しますか？コンテナ、イメージ、ボリュームも失われます。",
    ),
    (Msg::FilterHint, "Filter by name", "名前で絞り込み"),
    (Msg::NoContainers, "No containers", "コンテナがありません"),
    (
        Msg::ConfirmDeleteContainer,
        "Delete {name}?",
        "{name} を削除しますか？",
    ),
    (
        Msg::ConfirmForceDeleteContainer,
        "Stop and delete {name}?",
        "{name} を停止して削除しますか？",
    ),
    (Msg::Follow, "Follow", "追従"),
    (Msg::Close, "Close", "閉じる"),
    (Msg::InspectTitle, "Inspect", "詳細（inspect）"),
    (
        Msg::UsageLine,
        "CPU {cpu}% / memory {mem} of {limit}",
        "CPU {cpu}% / メモリ {mem}（上限 {limit}）",
    ),
    (Msg::RefreshUsage, "Refresh usage", "使用量を更新"),
    (Msg::RunTitle, "Run a container", "コンテナを実行"),
    (Msg::FieldImage, "Image", "イメージ名"),
    (
        Msg::FieldPorts,
        "Ports (for example 8080:80, 5353:53/udp)",
        "ポート（例 8080:80, 5353:53/udp）",
    ),
    (Msg::Run, "Run", "実行"),
    (
        Msg::TerminalFailed,
        "cannot open the terminal",
        "ターミナルを開けません",
    ),
    (Msg::PullTitle, "Pull an image", "イメージを取得（pull）"),
    (Msg::Pull, "Pull", "取得"),
    (Msg::NoImages, "No images", "イメージはありません"),
    (Msg::NoVolumes, "No volumes", "ボリュームはありません"),
    (Msg::NoNetworks, "No networks", "ネットワークはありません"),
    (Msg::Cleanup, "Clean up…", "片付け…"),
    (
        Msg::CleanupImagesTitle,
        "Clean up images",
        "イメージを片付ける",
    ),
    (Msg::CleanupImagesNone, "Keep images", "イメージは消さない"),
    (
        Msg::CleanupImagesDangling,
        "Remove dangling images only",
        "dangling のイメージだけ消す",
    ),
    (
        Msg::CleanupImagesUnused,
        "Remove all unused images",
        "未使用のイメージをすべて消す",
    ),
    (
        Msg::CleanupBuildCache,
        "Remove the build cache ({size})",
        "ビルドキャッシュを消す（{size}）",
    ),
    (Msg::Unknown, "unknown", "不明"),
    (
        Msg::ForceDeleteImage,
        "Delete even if in use (force)",
        "使用中でも削除（force）",
    ),
    (
        Msg::ConfirmDeleteImage,
        "Delete image {name}?",
        "イメージ {name} を削除しますか？",
    ),
    (
        Msg::ConfirmDeleteVolume,
        "Delete volume {name}?",
        "ボリューム {name} を削除しますか？",
    ),
    (
        Msg::ConfirmDeleteNetwork,
        "Delete network {name}?",
        "ネットワーク {name} を削除しますか？",
    ),
    (
        Msg::ConfirmPruneVolumes,
        "Delete {n} unused anonymous volumes ({size})?",
        "未使用の無名ボリューム {n} 件（{size}）を削除しますか？",
    ),
    (
        Msg::ConfirmPruneNetworks,
        "Delete all unused networks?",
        "未使用のネットワークをすべて削除しますか？",
    ),
    (Msg::Freed, "Freed {size}", "{size} を空けました"),
    (
        Msg::NetworksRemoved,
        "Removed {n} networks",
        "ネットワークを {n} 件削除しました",
    ),
    (Msg::NewVolume, "Create a volume", "ボリュームを作成"),
    (Msg::NewNetwork, "Create a network", "ネットワークを作成"),
    (Msg::FieldDriver, "Driver", "ドライバ"),
    (Msg::BuiltinNetwork, "built-in", "組み込み"),
    (Msg::RowTag, "Tag", "タグ付け"),
    (Msg::RowInspect, "Inspect", "詳細"),
    (Msg::ApplyTag, "Add tag", "タグを付ける"),
    (Msg::FieldRepo, "Repository", "リポジトリ"),
    (Msg::FieldTagName, "Tag name", "タグ名"),
    (Msg::DashMoreInstances, "and {n} more", "ほか {n} 件"),
    (
        Msg::FieldEnv,
        "Environment (KEY=VALUE per line)",
        "環境変数（1行に KEY=VALUE）",
    ),
    (
        Msg::FieldMounts,
        "Mounts (SOURCE:/path[:ro] per line)",
        "マウント（1行に ソース:/パス[:ro]）",
    ),
    (Msg::FieldRestart, "Restart policy", "再起動ポリシー"),
    (Msg::FieldCommand, "Command", "コマンド"),
    (
        Msg::DockerEnv,
        "invalid environment variable (use KEY=VALUE)",
        "環境変数の形が不正です（KEY=VALUE で書いてください）",
    ),
    (
        Msg::DockerMount,
        "invalid mount (use SOURCE:/container/path[:ro])",
        "マウントの形が不正です（ソース:/コンテナ内のパス[:ro] で書いてください）",
    ),
    (
        Msg::DockerVolumeNameTooLong,
        "Volume name is too long ({max} characters at most)",
        "ボリュームの名前が長すぎます（{max} 字まで）",
    ),
    (Msg::RowRename, "Rename", "名前を変更"),
    (Msg::FieldNewName, "New name", "新しい名前"),
    (Msg::ApplyRename, "Apply", "変更する"),
    (Msg::FieldVmType, "VM type", "VMの種類"),
    (Msg::FieldArch, "Architecture", "アーキテクチャ"),
    (Msg::FieldMountType, "Mount type", "マウント方式"),
    (
        Msg::ColimaSucceeded,
        "{profile}: colima {action} finished",
        "{profile}: colima {action} が終わりました",
    ),
    (
        Msg::Untagged,
        "Removed tag {name}",
        "タグ {name} を外しました",
    ),
    (Msg::DeletedItem, "Deleted {name}", "{name} を削除しました"),
    (
        Msg::ContainerDeleted,
        "Deleted the container",
        "コンテナを削除しました",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_are_bcp47_language_codes() {
        assert_eq!(Lang::En.tag(), "en");
        assert_eq!(Lang::Ja.tag(), "ja");
    }

    #[test]
    fn locale_mapping() {
        assert_eq!(Lang::from_locale("ja"), Lang::Ja);
        assert_eq!(Lang::from_locale("ja-JP"), Lang::Ja);
        assert_eq!(Lang::from_locale("en-US"), Lang::En);
        assert_eq!(Lang::from_locale("fr-FR"), Lang::En);
        assert_eq!(Lang::from_locale(""), Lang::En);
        assert_eq!(Lang::from_locale("jam-JM"), Lang::En);
        assert_eq!(Lang::from_locale("JA-jp"), Lang::Ja);
        assert_eq!(Lang::from_locale("ja_JP"), Lang::Ja);
    }

    #[test]
    fn env_override_wins_over_locale() {
        assert_eq!(Lang::resolve(Some("en"), Some("ja-JP")), Lang::En);
        assert_eq!(Lang::resolve(Some("ja"), Some("en-US")), Lang::Ja);
        assert_eq!(Lang::resolve(None, Some("ja-JP")), Lang::Ja);
        assert_eq!(Lang::resolve(None, None), Lang::En);
        // 知らない値と空の値は無視してOSの言語に従う。
        assert_eq!(Lang::resolve(Some("de"), Some("ja-JP")), Lang::Ja);
        assert_eq!(Lang::resolve(Some(""), Some("ja-JP")), Lang::Ja);
        assert_eq!(Lang::resolve(Some("EN"), Some("ja-JP")), Lang::En);
        assert_eq!(Lang::resolve(Some("Ja"), Some("en-US")), Lang::Ja);
    }

    #[test]
    fn override_parsing_is_case_insensitive_and_rejects_others() {
        assert_eq!(Lang::parse_override("en"), Some(Lang::En));
        assert_eq!(Lang::parse_override("EN"), Some(Lang::En));
        assert_eq!(Lang::parse_override("Ja"), Some(Lang::Ja));
        assert_eq!(Lang::parse_override("jp"), None);
        assert_eq!(Lang::parse_override("ja-JP"), None);
        assert_eq!(Lang::parse_override(""), None);
    }

    #[test]
    fn every_message_has_text_in_both_languages() {
        for m in Msg::ALL {
            assert!(!m.text(Lang::En).is_empty(), "{m:?} en");
            assert!(!m.text(Lang::Ja).is_empty(), "{m:?} ja");
            assert_ne!(m.text(Lang::En), m.text(Lang::Ja), "{m:?}");
        }
    }
}
