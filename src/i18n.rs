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
        if let Some(v) = env.as_deref()
            && !v.is_empty()
            && Lang::parse_override(v).is_none()
        {
            eprintln!("wharfkeep: WHARFKEEP_LANG={v} は使えません（en か ja）。OSの言語を使います");
        }
        Lang::resolve(env.as_deref(), sys_locale::get_locale().as_deref())
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
}

impl Msg {
    #[cfg(test)]
    const ALL: [Msg; 4] = [Msg::OpenWindow, Msg::Quit, Msg::Reload, Msg::ReloadCount];

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
