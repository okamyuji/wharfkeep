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
    TrayFailed,
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
}

impl Msg {
    #[cfg(test)]
    const ALL: [Msg; 20] = [
        Msg::OpenWindow,
        Msg::Quit,
        Msg::Reload,
        Msg::ReloadCount,
        Msg::UnknownSubcommand,
        Msg::NoHomeDir,
        Msg::LockFailed,
        Msg::TrayFailed,
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
            (Msg::TrayFailed, Lang::En) => "cannot add the menu bar item",
            (Msg::TrayFailed, Lang::Ja) => "メニューバーに項目を出せません",
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
            (Msg::ColimaProfile, Lang::En) => "invalid profile name (use A-Z a-z 0-9 . _ - only)",
            (Msg::ColimaProfile, Lang::Ja) => {
                "プロフィール名が不正です（使える文字は A-Z a-z 0-9 . _ - だけ）"
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
