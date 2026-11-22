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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    OpenWindow,
    Quit,
    Reload,
    ReloadCount,
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
}

impl Msg {
    #[cfg(test)]
    const ALL: [Msg; 42] = [
        Msg::OpenWindow,
        Msg::Quit,
        Msg::Reload,
        Msg::ReloadCount,
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
    ];

    pub fn text(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (Msg::OpenWindow, Lang::En) => "Open Window",
            (Msg::OpenWindow, Lang::Ja) => "ウィンドウを開く",
            (Msg::Quit, Lang::En) => "Quit",
            (Msg::Quit, Lang::Ja) => "終了",
            (Msg::Reload, Lang::En) => "Reload",
            (Msg::Reload, Lang::Ja) => "再読み込み",
            (Msg::ReloadCount, Lang::En) => "Reloads:",
            (Msg::ReloadCount, Lang::Ja) => "再読み込みした回数:",
            (Msg::UnknownSubcommand, Lang::En) => "unknown subcommand (use daemon or gui)",
            (Msg::UnknownSubcommand, Lang::Ja) => {
                "不明なサブコマンドです（daemon か gui を指定してください）"
            }
            (Msg::NoHomeDir, Lang::En) => {
                "cannot find the home directory, so the data directory is unknown"
            }
            (Msg::NoHomeDir, Lang::Ja) => {
                "ホームディレクトリが分からないため、データの置き場所を決められません"
            }
            (Msg::LockFailed, Lang::En) => "cannot take the lock",
            (Msg::LockFailed, Lang::Ja) => "ロックを取れません",
            (Msg::RuntimeFailed, Lang::En) => "cannot start the monitoring thread",
            (Msg::RuntimeFailed, Lang::Ja) => "監視のスレッドを始められません",
            (Msg::OpenWindowFailed, Lang::En) => "cannot open the window",
            (Msg::OpenWindowFailed, Lang::Ja) => "ウィンドウを開けません",
            (Msg::EventLoopCreateFailed, Lang::En) => "cannot create the event loop",
            (Msg::EventLoopCreateFailed, Lang::Ja) => "イベントループを作れません",
            (Msg::EventLoopStopped, Lang::En) => "the event loop stopped",
            (Msg::EventLoopStopped, Lang::Ja) => "イベントループが止まりました",
            (Msg::LangEnvInvalid, Lang::En) => {
                "unsupported value (use en or ja); using the OS language"
            }
            (Msg::LangEnvInvalid, Lang::Ja) => "使えない値です（en か ja）。OSの言語を使います",
            (Msg::ColimaParse, Lang::En) => "cannot read the colima output at line",
            (Msg::ColimaParse, Lang::Ja) => "colima の出力を読めません（行）",
            (Msg::ColimaSocket, Lang::En) => "unexpected docker socket address",
            (Msg::ColimaSocket, Lang::Ja) => "docker のソケットのアドレスが想定外です",
            (Msg::ColimaProfile, Lang::En) => {
                "invalid profile name (use A-Z a-z 0-9 . _ - only, not starting with -)"
            }
            (Msg::ColimaProfile, Lang::Ja) => {
                "プロフィール名が不正です（使える文字は A-Z a-z 0-9 . _ - だけ。先頭に - は使えません）"
            }
            (Msg::ColimaCpus, Lang::En) => "CPU count must be 1 or more",
            (Msg::ColimaCpus, Lang::Ja) => "CPU 数は 1 以上にしてください",
            (Msg::ColimaMemory, Lang::En) => "memory must be a finite number above 0 (GiB)",
            (Msg::ColimaMemory, Lang::Ja) => "メモリは 0 より大きい有限の数（GiB）にしてください",
            (Msg::ColimaDisk, Lang::En) => "disk size must be 1 GiB or more",
            (Msg::ColimaDisk, Lang::Ja) => "ディスクは 1 GiB 以上にしてください",
            (Msg::ColimaSpawn, Lang::En) => "cannot run colima",
            (Msg::ColimaSpawn, Lang::Ja) => "colima を実行できません",
            (Msg::ColimaFailed, Lang::En) => "colima failed (exit code)",
            (Msg::ColimaFailed, Lang::Ja) => "colima が失敗しました（終了コード）",
            (Msg::ColimaOutputTooLarge, Lang::En) => "colima printed too much output (over 1 MiB)",
            (Msg::ColimaOutputTooLarge, Lang::Ja) => "colima の出力が大きすぎます（1 MiB 超）",
            (Msg::ColimaTimeout, Lang::En) => "colima did not answer within 10 seconds",
            (Msg::ColimaTimeout, Lang::Ja) => "colima が 10 秒以内に応答しません",
            (Msg::SnapshotIo, Lang::En) => "cannot read the state file",
            (Msg::SnapshotIo, Lang::Ja) => "状態ファイルを読めません",
            (Msg::SnapshotParse, Lang::En) => "the state file is not valid",
            (Msg::SnapshotParse, Lang::Ja) => "状態ファイルの形が不正です",
            (Msg::SnapshotTooLarge, Lang::En) => "the state file is too large (over 16 MiB)",
            (Msg::SnapshotTooLarge, Lang::Ja) => "状態ファイルが大きすぎます（16 MiB 超）",
            (Msg::DockerConnect, Lang::En) => "cannot connect to the docker socket",
            (Msg::DockerConnect, Lang::Ja) => "docker のソケットに接続できません",
            (Msg::DockerApi, Lang::En) => "docker returned an error",
            (Msg::DockerApi, Lang::Ja) => "docker がエラーを返しました",
            (Msg::DockerPort, Lang::En) => {
                "invalid port mapping (use HOST:CONTAINER or HOST:CONTAINER/udp, ports 1-65535)"
            }
            (Msg::DockerPort, Lang::Ja) => {
                "ポートの対応が不正です（HOST:CONTAINER か HOST:CONTAINER/udp の形で、ポートは 1〜65535）"
            }
            (Msg::DockerInvalidId, Lang::En) => {
                "invalid name, ID or image reference (container, volume and network names: A-Z a-z 0-9 . _ - only, 2 characters or more, not starting with . or -)"
            }
            (Msg::DockerInvalidId, Lang::Ja) => {
                "名前、ID、イメージ参照が不正です（コンテナ、ボリューム、ネットワークの名前は A-Z a-z 0-9 . _ - だけ。2 文字以上で、先頭に . と - は使えません）"
            }
            (Msg::StartInstance, Lang::En) => "Start {name}",
            (Msg::StartInstance, Lang::Ja) => "{name} を起動",
            (Msg::StopInstance, Lang::En) => "Stop {name}",
            (Msg::StopInstance, Lang::Ja) => "{name} を停止",
            (Msg::StartAtLogin, Lang::En) => "Start at Login",
            (Msg::StartAtLogin, Lang::Ja) => "ログイン時に起動",
            (Msg::IconError, Lang::En) => "wharfkeep: a container failed",
            (Msg::IconError, Lang::Ja) => "wharfkeep: 失敗したコンテナがあります",
            (Msg::IconNormal, Lang::En) => "wharfkeep: running",
            (Msg::IconNormal, Lang::Ja) => "wharfkeep: 実行中",
            (Msg::IconIdle, Lang::En) => "wharfkeep: no instance running",
            (Msg::IconIdle, Lang::Ja) => "wharfkeep: 実行中のインスタンスはありません",
            (Msg::InstanceNotRunning, Lang::En) => "the instance is not running",
            (Msg::InstanceNotRunning, Lang::Ja) => "インスタンスが動いていません",
            (Msg::DockerEventsEnded, Lang::En) => "the docker event stream ended",
            (Msg::DockerEventsEnded, Lang::Ja) => "docker のイベントの購読が切れました",
            (Msg::TrayActionFailed, Lang::En) => "the menu action failed",
            (Msg::TrayActionFailed, Lang::Ja) => "メニューの操作が失敗しました",
            (Msg::AutostartFailed, Lang::En) => "cannot update the login item",
            (Msg::AutostartFailed, Lang::Ja) => "ログイン時の起動を設定できません",
            (Msg::SettingsSaveFailed, Lang::En) => "cannot save the settings",
            (Msg::SettingsSaveFailed, Lang::Ja) => "設定を保存できません",
            (Msg::SnapshotWriteFailed, Lang::En) => "cannot write the state file",
            (Msg::SnapshotWriteFailed, Lang::Ja) => "状態ファイルを書けません",
            (Msg::TempCleanupFailed, Lang::En) => "cannot remove leftover temporary files",
            (Msg::TempCleanupFailed, Lang::Ja) => "残った一時ファイルを消せません",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
