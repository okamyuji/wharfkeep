//! System EventsのAXでメニューバーとウィンドウを読み、押し、撮る。
use super::*;

impl Ctx {
    pub(super) fn osa(&self, lines: &[&str]) -> Option<String> {
        self.osa_within(lines, OSA_LIMIT)
    }

    fn osa_within(&self, lines: &[&str], limit: Duration) -> Option<String> {
        let mut cmd = Command::new("osascript");
        lines.iter().for_each(|l| {
            cmd.args(["-e", l]);
        });
        limited(&mut cmd, limit)
    }

    /// デーモンとGUIは同じプロセス名なので、pidで引いた1つの式で問い合わせる。
    pub(super) fn ax(&self, pid: &str, script: &str) -> Option<String> {
        self.ax_within(pid, script, OSA_LIMIT)
    }

    fn ax_within(&self, pid: &str, script: &str, limit: Duration) -> Option<String> {
        let tell = format!(
            "tell application \"System Events\" to tell (first process whose unix id is {pid})"
        );
        self.osa_within(&[&tell, script, "end tell"], limit)
    }

    /// ほかのアプリが前面を奪うことがあるので、前面にしてから同じスクリプトの中でメニューバーを引く。
    pub(super) fn bar_command(&self, pid: &str, script: &str) -> Command {
        let mut cmd = Command::new("osascript");
        cmd.args([
            "-e",
            &format!("tell application \"{}\" to activate", self.front_app),
        ])
        .args(["-e", "delay 0.3"])
        .args([
            "-e",
            &format!(
                "tell application \"System Events\" to tell (first process whose unix id is {pid})"
            ),
        ])
        .args(["-e", script, "-e", "end tell"]);
        cmd
    }

    pub(super) fn bar(&self, pid: &str, script: &str) -> Option<String> {
        limited(&mut self.bar_command(pid, script), OSA_LIMIT)
    }

    pub(super) fn activate_front_app(&self) {
        let _ = self.osa(&[&format!(
            "tell application \"{}\" to activate",
            self.front_app
        )]);
    }

    pub(super) fn item_geom(&self, pid: &str) -> Option<[i32; 4]> {
        let script = r#"tell menu bar item 1 of menu bar 2
if subrole is not "AXMenuExtra" then error "not a status item"
set {x, y} to position
set {w, h} to size
end tell
return (x as text) & " " & (y as text) & " " & (w as text) & " " & (h as text)"#;
        judge::parse_geom(&self.bar(pid, script)?)
    }

    pub(super) fn status_item_visible(&self, pid: &str) -> bool {
        self.item_geom(pid).is_some()
    }

    /// VoiceOverが読むアイコンの説明。色が見えなくても状態が分かる。
    pub(super) fn icon_desc(&self, pid: &str) -> String {
        self.bar(pid, "get description of menu bar item 1 of menu bar 2")
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    pub(super) fn menu_open_below(&self, pid: &str, item: [i32; 4]) -> bool {
        self.ax(
            pid,
            "get position of menu 1 of menu bar item 1 of menu bar 2",
        )
        .and_then(|p| judge::parse_pos(&p))
        .is_some_and(|pos| judge::menu_open_below(pos, item))
    }

    pub(super) fn no_status_item(&self) -> String {
        format!(
            "メニューバー項目がAXに見えません。{}を外部画面に置いてください（内蔵画面ではノッチの裏に隠れる）",
            self.front_app
        )
    }

    /// ステータス項目をAXPressで押してメニューを開き、項目を選ぶ。VoiceOverがメニューを開くのと同じ経路。
    /// メニューが開いたあとで閉じることがあるので、閉じたら開き直して選び直す。
    pub(super) fn choose_menu_item(&self, pid: &str, item: &str, stage: &str) {
        let geom = self
            .item_geom(pid)
            .unwrap_or_else(|| self.fail(&self.no_status_item()));
        let target = judge::menu_target(item);
        let mut clicks = Vec::new();
        let chosen = (0..3).any(|_| self.try_menu(pid, geom, &target, stage, &mut clicks));
        self.need(chosen, || {
            format!("メニューを3回開いても「{item}」を押せません")
        });
        // 自分が起こした押下だけを、上限つきで待つ。
        clicks
            .into_iter()
            .for_each(|c| reap(c, Duration::from_secs(10)));
    }

    pub(super) fn try_menu(
        &self,
        pid: &str,
        [ix, iy, iw, ih]: [i32; 4],
        target: &str,
        stage: &str,
        clicks: &mut Vec<Child>,
    ) -> bool {
        // 開いたメニューの中で操作が終わるまでAXPressが戻らないことがあるので、待たずに先へ進む。
        let click = self
            .bar_command(pid, "click menu bar item 1 of menu bar 2")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        clicks.extend(click);
        self.wait(5, || self.menu_open_below(pid, [ix, iy, iw, ih])) && {
            self.shot_rect(stage, &format!("{},{iy},600,220", ix + iw / 2 - 300));
            let script = format!("click {target} of menu 1 of menu bar item 1 of menu bar 2");
            self.ax(pid, &script).is_some()
        }
    }

    /// eframeは隠れたウィンドウのUIを走らせないので、GUIを前面に戻してから読む。
    pub(super) fn contents(&self, pid: &str) -> String {
        self.ax(
            pid,
            "if not frontmost then set frontmost to true\nget entire contents of window \"wharfkeep\"",
        )
        .unwrap_or_default()
    }

    pub(super) fn has_element(&self, pid: &str, needle: &str) -> bool {
        judge::has_element(&self.contents(pid), needle)
    }

    pub(super) fn has_prefix(&self, pid: &str, needle: &str) -> bool {
        judge::has_prefix(&self.contents(pid), needle)
    }

    pub(super) fn window_rect(&self, pid: &str) -> String {
        let script = r#"tell window "wharfkeep"
set {x, y} to position
set {w, h} to size
end tell
return (x as text) & "," & (y as text) & "," & (w as text) & "," & (h as text)"#;
        self.ax(pid, script).unwrap_or_default().trim().to_string()
    }

    /// 種類と名前の要素に`verb`を当てる。探し方は`judge::ax_script`。
    /// 入れ子の奥まで探すと要素の数だけ問い合わせるので、上限を長めにする。
    fn ax_each(&self, pid: &str, verb: &str, kind: &str, name: &str, tail: &str) -> bool {
        let script = judge::ax_script(verb, kind, name, tail);
        self.ax_within(pid, &script, AX_SEARCH_LIMIT).is_some()
    }

    /// ウィンドウ内の要素を種類と名前で押す。例: `ax_press(pid, "checkbox", "Containers")`。
    pub(super) fn ax_press(&self, pid: &str, kind: &str, name: &str) -> bool {
        self.ax_each(pid, "click", kind, name, "")
    }

    /// 名前の付いた入力欄に値を入れる。eguiのTextEditはAccessKitのSetValueを受ける。
    pub(super) fn ax_set(&self, pid: &str, field: &str, value: &str) -> bool {
        self.ax_each(
            pid,
            "set value of",
            "text field",
            field,
            &judge::to_value(value),
        )
    }

    pub(super) fn press(&self, pid: &str, kind: &str, name: &str) {
        let pressed = self.ax_press(pid, kind, name);
        self.need(pressed, || {
            format!("「{name}」を押せません。{}", self.tree(pid))
        });
    }

    pub(super) fn front_pid(&self) -> String {
        self.osa(&["tell application \"System Events\" to get unix id of first process whose frontmost is true"])
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    pub(super) fn is_front(&self, pid: &str) -> bool {
        self.front_pid() == pid
    }

    pub(super) fn window_count(&self, pid: &str) -> String {
        self.ax(pid, "count windows")
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    pub(super) fn close_window(&self, pid: &str) {
        let _ = self.ax(
            pid,
            "click (first button of window \"wharfkeep\" whose subrole is \"AXCloseButton\")",
        );
        let gone = self.wait(10, || self.guis_are(0));
        self.need(gone, || {
            "ウィンドウを閉じてもGUIのプロセスが残ります".into()
        });
    }

    /// サイドバーの項目を押し、同じ名前の見出しが出ることで、eguiが押されたことを受け取ったと確かめる。
    pub(super) fn check_navigation(&self, gpid: &str, page: &str, other: &str) {
        let seen = self.wait(15, || self.has_element(gpid, &format!("checkbox {page}")));
        self.need(seen, || format!("サイドバーの「{page}」がAXで見えません"));
        let before = self.has_element(gpid, &format!("UI element {other}"));
        self.need(before, || format!("押す前の見出し「{other}」がありません"));
        let _ = self.ax_press(gpid, "checkbox", page);
        let after = self.wait(5, || self.has_element(gpid, &format!("UI element {page}")));
        self.need(after, || format!("押した後の見出し「{page}」が出ません"));
    }

    /// 要素が出るまで待つ。出なければ`msg`で失敗する。
    pub(super) fn see(&self, secs: u64, gpid: &str, needle: &str, msg: &str) {
        let seen = self.wait(secs, || self.has_element(gpid, needle));
        self.need(seen, || format!("{msg}。{}", self.tree(gpid)));
    }

    /// 失敗の文に添える、今のAXの木の要素の名前。次に失敗したとき、名前の違いか、まだ無いのかを見分ける。
    pub(super) fn tree(&self, pid: &str) -> String {
        format!("AXの要素: {}", judge::names(&self.contents(pid), 3000))
    }

    // ---- 画面 ----

    pub(super) fn shot(&self, name: &str) {
        let _ = ok(Command::new("screencapture")
            .arg("-x")
            .arg(self.out.join(format!("{name}.png"))));
    }

    /// 画像は目で見る記録で、合否はAXと状態ファイルで決める。撮れなくても失敗にしない。
    pub(super) fn shot_rect(&self, name: &str, rect: &str) {
        let path = self.out.join(format!("{name}.png"));
        let taken = ok(Command::new("screencapture")
            .args(["-x", "-R", rect])
            .arg(path));
        if !taken {
            eprintln!("e2e: 警告: {name} の画像を撮れません（範囲 {rect}）");
        }
    }

    pub(super) fn shot_window(&self, name: &str, gpid: &str) {
        self.shot_rect(name, &self.window_rect(gpid));
    }
}
