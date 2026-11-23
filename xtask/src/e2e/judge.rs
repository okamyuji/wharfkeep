//! e2eの判断。AXやlaunchctlが返した文字列を読み、待ちを打ち切るかを決める。プロセスは起こさない。
use std::time::Duration;

/// `E2E_LIMIT`（秒）。無ければ1500秒。
pub fn limit_secs(env: Option<&str>) -> Result<u64, String> {
    match env {
        None => Ok(1500),
        Some(v) => v
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| format!("E2E_LIMIT={v} は正の整数の秒数ではありません")),
    }
}

/// pgrepの`-x`の正規表現として、パスの「.」などを記号として読ませない。
pub fn regex_escape(s: &str) -> String {
    s.chars().fold(String::new(), |mut out, c| {
        if "[]\\.*^$+?(){}|".contains(c) {
            out.push('\\');
        }
        out.push(c);
        out
    })
}

/// `ioreg -n Root -d1`の出力。ロック中はmacOSがウィンドウを描かず、AXからGUIの中身が見えない。
pub fn screen_locked(ioreg: &str) -> bool {
    ioreg.contains("\"CGSSessionScreenIsLocked\"=Yes")
}

/// `launchctl print`の最初の`pid = N`。動いていなければNone。
pub fn launchd_pid(print: &str) -> Option<u32> {
    print
        .lines()
        .find_map(|l| match l.split_whitespace().collect::<Vec<_>>()[..] {
            ["pid", "=", n] => n.parse().ok(),
            _ => None,
        })
}

/// 空でない行の数。pgrepの出力をpidの数として数える。
pub fn count_lines(s: &str) -> usize {
    s.lines().filter(|l| !l.trim().is_empty()).count()
}

/// AXの`entire contents`は要素を「, 」で区切って返す。要素ごとに分ける。
fn elements(contents: &str) -> impl Iterator<Item = &str> {
    contents.split(',')
}

/// 名前の途中までで探す。例: `has_prefix(c, "button Running: ")`。
pub fn has_prefix(contents: &str, needle: &str) -> bool {
    let needle = format!(" {needle}");
    elements(contents).any(|e| e.contains(&needle))
}

/// 種類と名前が一致する要素があるか。例: `has_element(c, "button Reload")`。
pub fn has_element(contents: &str, needle: &str) -> bool {
    let needle = format!(" {needle} of ");
    elements(contents).any(|e| e.contains(&needle))
}

/// 名前の途中から、AXの完全な名前を取る。例: `elem_name(c, "checkbox", "Remove the build cache")`。
pub fn elem_name(contents: &str, kind: &str, part: &str) -> Option<String> {
    let head = format!("{kind} ");
    elements(contents)
        .map(strip_location)
        .filter_map(|e| e.trim_start().strip_prefix(&head))
        .find(|name| name.contains(part))
        .map(str::to_string)
}

/// 場所の部分（「 of UI element 3 of group 1 of window …」や確認ダイアログの「 of window 1 of group 1 …」）を外す。
fn strip_location(e: &str) -> &str {
    let cut = ["UI element", "window"].iter().find_map(|kind| {
        let marker = format!(" of {kind} ");
        e.match_indices(&marker).map(|(i, _)| i).find(|&i| {
            let rest = &e[i + marker.len()..];
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            digits > 0 && rest[digits..].starts_with(" of group 1 of window ")
        })
    });
    cut.map_or(e, |i| &e[..i])
}

/// 「x y w h」。メニューバー項目の位置と大きさ。
pub fn parse_geom(s: &str) -> Option<[i32; 4]> {
    let v: Vec<i32> = s
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    v.try_into().ok()
}

/// 「x, y」。AXの位置。
pub fn parse_pos(s: &str) -> Option<(i32, i32)> {
    let (x, y) = s.trim().split_once(',')?;
    Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
}

/// メニューを閉じていてもAXはメニューの項目を返すので、メニューが項目の真下にあることで開いたと見なす。
pub fn menu_open_below((mx, my): (i32, i32), [ix, iy, _, ih]: [i32; 4]) -> bool {
    my >= iy + ih && my <= iy + ih + 20 && mx > ix - 300 && mx < ix + 300
}

/// メニューの項目のAppleScriptの参照。「last」は終了、「login」はその1つ前のログイン時に起動。
pub fn menu_target(item: &str) -> String {
    match item {
        "last" => "last menu item".into(),
        "login" => "menu item -2".into(),
        _ => format!("menu item \"{item}\""),
    }
}

/// `docker system df --format '{{.Type}}|{{.Size}}'`でビルドキャッシュが空か。
pub fn cache_empty(df: &str) -> bool {
    df.lines().any(|l| l.trim() == "Build Cache|0B")
}

/// 外部ターミナルでGUIが開いたシェル（`docker exec -it <id> sh`）を探す`pgrep -f`の型。
/// GUIは完全なIDか短いIDを渡すので、短いIDから始まる16進の並びで受ける。
pub fn shell_pattern(id: &str) -> String {
    let short: String = id.chars().take(12).collect();
    format!("exec -it {short}[0-9a-f]* sh$")
}

/// `ps -o tty=`の値をTerminal.appのタブの`tty`の形にする。端末が無ければNone。
pub fn tty_path(ps: &str) -> Option<String> {
    let t = ps.trim();
    (!t.is_empty() && t != "??").then(|| format!("/dev/{t}"))
}

/// 段を終えたか（`done`）と、後片付けでログイン時の起動先を戻せたか（`restored`）から、結果を決める。
pub fn outcome(done: bool, restored: bool, stage: &str) -> Result<&'static str, String> {
    match (done, restored) {
        (true, true) => Ok("e2e: 成功"),
        (true, false) => Err("e2e: 後片付けでログイン時の起動先を戻せません".into()),
        (false, _) => Err(format!("e2e: 段「{stage}」で止まりました")),
    }
}

/// AppleScriptの文字列。`"`と`\`を逃がす。
pub fn to_value(value: &str) -> String {
    format!(
        " to \"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

/// 段を走らせた子プロセスの終了コードから、親の結果を決める。子は失敗の理由を自分で書いている。
pub fn child_outcome(code: Option<i32>) -> Result<(), String> {
    match code {
        Some(0) => Ok(()),
        Some(n) => Err(format!("e2e が失敗しました（終了コード {n}）")),
        None => Err("e2e がシグナルで終わりました".into()),
    }
}

/// 見張りが止める理由。親が消えた（Ctrl-Cで中断された）か、上限の秒数を超えたか。
pub fn stop_reason(elapsed: Duration, limit: u64, parent_alive: bool) -> Option<String> {
    if !parent_alive {
        return Some("中断されました".into());
    }
    (elapsed >= Duration::from_secs(limit)).then(|| format!("上限 {limit} 秒を超えました"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // `entire contents of window "wharfkeep"`の出力を縮めたもの。
    const CONTENTS: &str = "group 1 of window wharfkeep of application process wharfkeep, checkbox Containers of UI element 2 of group 1 of window wharfkeep of application process wharfkeep, UI element Dashboard of UI element 3 of group 1 of window wharfkeep of application process wharfkeep, button Running: 2 of UI element 3 of group 1 of window wharfkeep of application process wharfkeep, checkbox Remove the build cache (1.2 GB) of window 1 of group 1 of window wharfkeep of application process wharfkeep, static text Running of UI element 4 of group 1 of window wharfkeep of application process wharfkeep";

    #[test]
    fn limit_secs_defaults_and_refuses_bad_values() {
        assert_eq!(limit_secs(None), Ok(1500));
        assert_eq!(limit_secs(Some("60")), Ok(60));
        for bad in ["0", "-1", "1.5", "", "x"] {
            assert!(limit_secs(Some(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn regex_escape_quotes_every_metacharacter() {
        assert_eq!(
            regex_escape("/a.app/x[1]+(y)|z*^$?{}\\"),
            "/a\\.app/x\\[1\\]\\+\\(y\\)\\|z\\*\\^\\$\\?\\{\\}\\\\"
        );
        assert_eq!(regex_escape("plain/path"), "plain/path");
    }

    #[test]
    fn screen_locked_reads_the_session_flag() {
        assert!(screen_locked("  \"CGSSessionScreenIsLocked\"=Yes\n"));
        assert!(!screen_locked("  \"CGSSessionScreenIsLocked\"=No\n"));
        assert!(!screen_locked(""));
    }

    #[test]
    fn launchd_pid_takes_the_first_pid_line() {
        let print = "gui/501/wharfkeep = {\n\tstate = running\n\tpid = 52399\n\tsubsystem = {\n\t\tpid = 1\n\t}\n}\n";
        assert_eq!(launchd_pid(print), Some(52399));
        assert_eq!(launchd_pid("\tstate = not running\n"), None);
        assert_eq!(launchd_pid("\tpid = x\n"), None);
        assert_eq!(launchd_pid("\tppid = 3\n\tpid = 4 5\n"), None);
    }

    #[test]
    fn count_lines_ignores_blank_lines() {
        assert_eq!(count_lines("12\n34\n"), 2);
        assert_eq!(count_lines("\n  \n"), 0);
        assert_eq!(count_lines(""), 0);
    }

    #[test]
    fn has_element_needs_the_whole_name_and_has_prefix_does_not() {
        assert!(has_element(CONTENTS, "checkbox Containers"));
        assert!(has_element(CONTENTS, "UI element Dashboard"));
        assert!(!has_element(CONTENTS, "checkbox Contain"));
        assert!(!has_element(CONTENTS, "button Stop web"));
        assert!(has_prefix(CONTENTS, "button Running: "));
        assert!(!has_prefix(CONTENTS, "button Stopped: "));
        // 要素の区切りをまたいで当てない。
        assert!(!has_prefix(CONTENTS, "wharfkeep checkbox"));
    }

    #[test]
    fn elem_name_returns_the_full_name_without_the_location() {
        assert_eq!(
            elem_name(CONTENTS, "checkbox", "Remove the build cache"),
            Some("Remove the build cache (1.2 GB)".to_string())
        );
        assert_eq!(
            elem_name(CONTENTS, "button", "Running"),
            Some("Running: 2".to_string())
        );
        assert_eq!(elem_name(CONTENTS, "radio button", "Remove"), None);
        assert_eq!(elem_name(CONTENTS, "checkbox", "Volumes"), None);
    }

    #[test]
    fn strip_location_keeps_names_that_only_look_like_a_location() {
        assert_eq!(
            strip_location("button Open of window x of group 1 of window wharfkeep"),
            "button Open of window x of group 1 of window wharfkeep"
        );
        assert_eq!(
            strip_location("button A of UI element 12 of group 1 of window w of app"),
            "button A"
        );
        // 添字の数字が無ければ場所ではない。
        assert_eq!(
            strip_location("button A of UI element  of group 1 of window w"),
            "button A of UI element  of group 1 of window w"
        );
        assert_eq!(
            strip_location("button A of UI element 2 of group 2 of window w"),
            "button A of UI element 2 of group 2 of window w"
        );
    }

    #[test]
    fn parse_geom_and_pos_read_numbers() {
        assert_eq!(parse_geom("1200 0 24 24"), Some([1200, 0, 24, 24]));
        assert_eq!(parse_geom("-5 3 1 2"), Some([-5, 3, 1, 2]));
        assert_eq!(parse_geom("1 2 3"), None);
        assert_eq!(parse_geom("1 2 3 4 5"), None);
        assert_eq!(parse_geom("1 2 x 4"), None);
        assert_eq!(parse_pos("1190, 24\n"), Some((1190, 24)));
        assert_eq!(parse_pos("1190,24"), Some((1190, 24)));
        assert_eq!(parse_pos("1190"), None);
        assert_eq!(parse_pos("a, 1"), None);
        assert_eq!(parse_pos("1, b"), None);
    }

    #[test]
    fn menu_open_below_accepts_only_a_menu_right_under_the_item() {
        let item = [1000, 0, 24, 24];
        assert!(menu_open_below((990, 24), item));
        assert!(menu_open_below((990, 44), item));
        assert!(!menu_open_below((990, 23), item));
        assert!(!menu_open_below((990, 45), item));
        assert!(menu_open_below((701, 30), item));
        assert!(!menu_open_below((700, 30), item));
        assert!(menu_open_below((1299, 30), item));
        assert!(!menu_open_below((1300, 30), item));
    }

    #[test]
    fn menu_target_picks_by_position_or_name() {
        assert_eq!(menu_target("last"), "last menu item");
        assert_eq!(menu_target("login"), "menu item -2");
        assert_eq!(menu_target("Open Window"), "menu item \"Open Window\"");
    }

    #[test]
    fn cache_empty_needs_a_zero_build_cache_line() {
        assert!(cache_empty("Images|1GB\nBuild Cache|0B\n"));
        assert!(!cache_empty("Build Cache|12MB\n"));
        assert!(!cache_empty("Build Cache|0B extra\n"));
        assert!(!cache_empty(""));
    }

    #[test]
    fn shell_pattern_matches_the_short_or_full_id() {
        let full = "0123456789abcdef0123";
        assert_eq!(shell_pattern(full), "exec -it 0123456789ab[0-9a-f]* sh$");
        assert_eq!(shell_pattern("abc"), "exec -it abc[0-9a-f]* sh$");
    }

    #[test]
    fn tty_path_needs_a_terminal() {
        assert_eq!(tty_path("ttys012\n"), Some("/dev/ttys012".into()));
        assert_eq!(tty_path("??"), None);
        assert_eq!(tty_path(" "), None);
    }

    #[test]
    fn outcome_needs_every_stage_and_the_restore() {
        assert_eq!(outcome(true, true, "x"), Ok("e2e: 成功"));
        assert!(outcome(true, false, "x").unwrap_err().contains("起動先"));
        for restored in [true, false] {
            let e = outcome(false, restored, "== web").unwrap_err();
            assert!(e.contains("== web"), "{e}");
        }
    }

    #[test]
    fn to_value_quotes_for_applescript() {
        assert_eq!(to_value("nginx:alpine"), " to \"nginx:alpine\"");
        assert_eq!(to_value("a\"b\\c"), " to \"a\\\"b\\\\c\"");
    }

    #[test]
    fn child_outcome_passes_only_a_zero_exit() {
        assert_eq!(child_outcome(Some(0)), Ok(()));
        assert!(child_outcome(Some(1)).unwrap_err().contains("終了コード 1"));
        assert!(child_outcome(None).unwrap_err().contains("シグナル"));
    }

    #[test]
    fn stop_reason_stops_on_interrupt_or_the_limit() {
        let s = Duration::from_secs;
        assert_eq!(stop_reason(s(5), 10, true), None);
        assert_eq!(
            stop_reason(s(10), 10, true),
            Some("上限 10 秒を超えました".into())
        );
        assert_eq!(
            stop_reason(s(11), 10, true),
            Some("上限 10 秒を超えました".into())
        );
        assert_eq!(stop_reason(s(0), 10, false), Some("中断されました".into()));
        assert_eq!(stop_reason(s(99), 10, false), Some("中断されました".into()));
    }
}
