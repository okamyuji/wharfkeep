use std::process::Command;

fn stderr_for(lang: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_wharfkeep"))
        .arg("bogus")
        .env("WHARFKEEP_LANG", lang)
        .output()
        .expect("run wharfkeep");
    assert_eq!(out.status.code(), Some(2));
    String::from_utf8(out.stderr).expect("utf-8 stderr")
}

#[test]
fn wharfkeep_lang_ja_selects_japanese_messages() {
    assert!(stderr_for("ja").contains("不明なサブコマンド"));
}

#[test]
fn wharfkeep_lang_en_selects_english_messages() {
    assert!(stderr_for("en").contains("unknown subcommand"));
}

#[test]
fn invalid_wharfkeep_lang_is_reported() {
    assert!(stderr_for("xx").contains("WHARFKEEP_LANG=xx"));
}

#[test]
fn empty_wharfkeep_lang_is_not_reported() {
    assert!(!stderr_for("").contains("WHARFKEEP_LANG="));
}
