//! 英語の通しの流れと、ログイン時の起動（launchd）の段。
use super::*;

impl Ctx {
    /// 何も変える前の確かめ。止まるときは後片付けもしない。
    pub(super) fn preflight(&self) -> Result<(), String> {
        let ioreg = stdout(Command::new("ioreg").args(["-n", "Root", "-d1"]));
        if judge::screen_locked(&ioreg) {
            return Err("画面がロックされています。ロックを解除してから実行してください".into());
        }
        if port_in_use(WEB_PORT) {
            return Err(format!(
                "ポート {WEB_PORT} が使用中です。コンテナの実行を確かめるので、使っているものを止めてください"
            ));
        }
        fs::create_dir_all(&self.out).map_err(|e| format!("{}: {e}", self.out.display()))
    }

    pub(super) fn start(&self) {
        // 前回の試験が残したlaunchdのデーモンも止める。launchdに残すと止めても起こし直される。
        let _ = self.launchctl(&["bootout", &self.service]);
        let _ = ok(Command::new("pkill").args(["-fx", &format!("{}( daemon| gui)?", self.exe_re)]));
        let gone = self.wait(10, || self.daemons_are(0));
        self.need(gone, || "前回のデーモンが残っています".into());
        let gone = self.wait(10, || self.guis_are(0));
        self.need(gone, || "前回のGUIが残っています".into());
        self.need(self.exe.is_file(), || {
            format!(
                "{} がありません。先に cargo xtask bundle --no-bump を実行してください",
                self.exe.display()
            )
        });
        self.need(self.colima_was_running, || {
            "colimaのインスタンス default を起動してから実行してください".into()
        });
        // 前回の状態ファイルで5秒の確かめが通ってしまわないよう消す。設定は既定（自動起動は有効）から始める。
        let _ = fs::remove_file(&self.state);
        let _ = fs::remove_file(self.data.join("settings.json"));
    }

    pub(super) fn dpid(&self) -> String {
        first(&self.daemon_pids())
    }

    pub(super) fn gpid(&self) -> String {
        first(&self.gui_pids())
    }

    pub(super) fn en_menu(&self) {
        // AXの名前を固定するため、英語で通しの流れを試す。
        self.say("== en");
        let _ = ok(Command::new("open")
            .args(["--env", "WHARFKEEP_LANG=en"])
            .arg(&self.app));
        let started = std::time::Instant::now();
        let one = self.wait(10, || self.daemons_are(1));
        self.need(one, || {
            format!(
                "デーモンが1つになりません: {}",
                self.daemon_pids().replace('\n', " ")
            )
        });
        let dpid = self.dpid();
        self.say(&format!("(1)(2) デーモン pid={dpid}"));
        let written = self.wait(5, || self.state_has_default());
        self.need(written, || {
            "5秒以内に状態ファイルへ default が書かれません".into()
        });
        self.say(&format!(
            "(state) {}秒以内に書かれた: {}",
            started.elapsed().as_secs(),
            self.show_state()
        ));
        self.autostart_registered();
        let seen = self.wait(10, || self.status_item_visible(&dpid));
        self.need(seen, || self.no_status_item());
        self.shot("en-1-daemon");
        self.choose_menu_item(&dpid, "Open Window", "en-3-menu");
        self.say("(3) AXPressで開いたメニューの「Open Window」を選んだ");
        let up = self.wait(10, || self.guis_are(1));
        self.need(up, || "GUIのプロセスが立ちません".into());
        let gpid = self.gpid();
        self.say(&format!("(4) GUI pid={gpid}"));
        self.check_navigation(&gpid, "Containers", "Dashboard");
        self.shot_window("en-5-gui", &gpid);
        self.say("(5) 「Containers」を押し、見出しが「Containers」になった");
    }

    /// 自動起動 (1) 初回の起動でlaunchdに登録される。
    pub(super) fn autostart_registered(&self) {
        let loaded = self.wait(10, || self.loaded());
        self.need(loaded, || {
            format!("launchctl print {} が失敗します", self.service)
        });
        let plist = fs::read_to_string(&self.plist).unwrap_or_default();
        self.need(plist.contains("<string>daemon</string>"), || {
            "plistに daemon の引数がありません".into()
        });
        let keep = "<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>";
        self.need(plist.contains(keep), || {
            "plistにKeepAliveがありません".into()
        });
        self.say(&format!(
            "(autostart-1) launchctl print {} が成功し、plistに daemon と KeepAlive がある",
            self.service
        ));
    }

    /// (5r) コンテナ画面の行の色帯、状態の文字、1回押すだけのボタン。
    pub(super) fn en_rows(&self) {
        let gpid = self.gpid();
        self.row_up();
        self.see(
            15,
            &gpid,
            &format!("button Stop {ROW}"),
            &format!("行の「Stop {ROW}」がAXで見えません"),
        );
        self.need(self.has_element(&gpid, "static text Running"), || {
            "行に状態の文字「Running」がありません".into()
        });
        self.shot_window("en-5r-row-running", &gpid);
        self.press(&gpid, "button", &format!("Stop {ROW}"));
        let exited = self.wait(60, || self.row_is("exited"));
        self.need(exited, || {
            format!(
                "「Stop {ROW}」を1回押しても {ROW} が止まりません: {}",
                self.row_state()
            )
        });
        self.see(
            15,
            &gpid,
            &format!("button Start {ROW}"),
            &format!("止まった行に「Start {ROW}」が出ません"),
        );
        self.need(self.has_element(&gpid, "static text Stopped"), || {
            "止まった行に状態の文字「Stopped」がありません".into()
        });
        self.shot_window("en-5r-row-stopped", &gpid);
        self.say(&format!(
            "(5r) 「Stop {ROW}」を1回押して {ROW} が exited になった"
        ));
        self.press(&gpid, "button", &format!("Start {ROW}"));
        let running = self.wait(60, || self.row_is("running"));
        self.need(running, || {
            format!(
                "「Start {ROW}」を1回押しても {ROW} が動きません: {}",
                self.row_state()
            )
        });
        self.see(
            15,
            &gpid,
            &format!("button Stop {ROW}"),
            &format!("動き直した行に「Stop {ROW}」が出ません"),
        );
        self.say(&format!(
            "(5r) 「Start {ROW}」を1回押して {ROW} が running に戻った"
        ));
        self.row_cleanup();
        let gone = self.wait(30, || self.row_is(""));
        self.need(gone, || format!("{ROW} を消しても状態ファイルに残ります"));
        let gone = self.wait(15, || !judge::has_prefix(&self.contents(&gpid), ROW));
        self.need(gone, || format!("{ROW} を消しても行が残ります"));
        self.say(&format!("(5r) {ROW} を消し、行も消えた"));
    }

    /// (5d) ダッシュボードの枠の内訳と、(5b) 二度目の「Open Window」。
    pub(super) fn en_dashboard_and_raise(&self) {
        let (dpid, gpid) = (self.dpid(), self.gpid());
        self.row_up();
        self.press(&gpid, "checkbox", "Dashboard");
        let n = self.jq_text(&[
            r#"[.docker.default.Ready.containers[] | select(.state == "running" and (.failed | not))] | length"#,
        ]);
        self.see(
            15,
            &gpid,
            &format!("static text Running: {n}"),
            &format!("コンテナの枠に内訳「Running: {n}」が出ません"),
        );
        self.need(self.has_prefix(&gpid, "button Containers: "), || {
            "ダッシュボードにコンテナの枠のボタンがありません".into()
        });
        self.need(self.has_prefix(&gpid, "button Instances: "), || {
            "ダッシュボードにインスタンスの枠のボタンがありません".into()
        });
        self.need(
            self.has_element(&gpid, "static text default: Running"),
            || "インスタンスの枠に「default: Running」がありません".into(),
        );
        self.shot_window("en-5d-dashboard", &gpid);
        self.say("(5d) ダッシュボードのインスタンスの枠とコンテナの枠に内訳が出た（色帯は画像で確かめる）");
        self.row_cleanup();
        let gone = self.wait(30, || self.row_is(""));
        self.need(gone, || format!("{ROW} を消しても状態ファイルに残ります"));
        self.press(&gpid, "checkbox", "Containers");
        // choose_menu_itemは別のアプリを前面にしてからメニューを開くので、前面に戻れば二度目の起動が働いた証になる。
        self.choose_menu_item(&dpid, "Open Window", "en-5b-menu-open-again");
        let front = self.wait(10, || self.is_front(&gpid));
        self.need(front, || {
            format!(
                "二度目の「Open Window」でGUIが前面に出ません（前面 pid={}）",
                self.front_pid()
            )
        });
        self.need(self.guis_are(1), || {
            format!(
                "二度目の「Open Window」でGUIが2つになりました: {}",
                self.gui_pids().replace('\n', " ")
            )
        });
        let windows = self.window_count(&gpid);
        self.need(windows == "1", || {
            format!("ウィンドウが1つではありません: {windows}")
        });
        self.shot("en-5b-front");
        self.say(&format!(
            "(5b) 二度目の「Open Window」で、GUI pid={gpid} のウィンドウ1つが前面に出た"
        ));
    }

    /// (5c) デーモンを止めると、GUIに停止の表示と「Start daemon」が出る。(6) 閉じるとGUIは終わりデーモンは残る。
    pub(super) fn en_daemon_stopped(&self) {
        let (dpid, gpid) = (self.dpid(), self.gpid());
        // GUIが前面のままメニューを開くと、前面の切り替えで閉じることがあるので、先に別のアプリを前面にする。
        self.activate_front_app();
        let moved = self.wait(10, || !self.is_front(&gpid));
        self.need(moved, || "GUIから前面が移りません".into());
        std::thread::sleep(Duration::from_secs(1));
        self.choose_menu_item(&dpid, "Quit", "en-5c-menu-quit");
        let quit = self.wait(10, || self.daemons_are(0));
        self.need(quit, || "「Quit」でデーモンが終わりません".into());
        // 他のアプリの裏に隠れたウィンドウは描き直されないので、二度目の起動で前面に出してから待つ。
        let _ = ok(Command::new(&self.exe).arg("gui"));
        let front = self.wait(10, || self.is_front(&gpid));
        self.need(front, || "GUIが前面に出ません".into());
        self.see(
            45,
            &gpid,
            "static text The daemon is not running",
            "デーモン停止の表示がGUIに出ません",
        );
        self.shot_window("en-5c-gui-stopped", &gpid);
        self.say("(5c) デーモンを止めると、GUIに「The daemon is not running」が出た");
        self.press(&gpid, "button", "Start daemon");
        let up = self.wait(15, || self.daemons_are(1));
        self.need(up, || "「Start daemon」でデーモンが立ちません".into());
        let dpid = self.dpid();
        let gone = self.wait(30, || {
            !self.contents(&gpid).contains("The daemon is not running")
        });
        self.need(gone, || "デーモンを起動しても停止の表示が消えません".into());
        self.say(&format!(
            "(5c) 「Start daemon」でデーモン pid={dpid} が立ち、停止の表示が消えた"
        ));
        self.close_window(&gpid);
        self.need(self.dpid() == dpid, || "デーモンが残っていません".into());
        self.shot("en-6-closed");
        self.say(&format!("(6) GUIは終了し、デーモン pid={dpid} は残った"));
    }

    /// (7) メニューからdefaultを止めて起動し直す。(8) 「Quit」でlaunchdの外のデーモンを止める。
    pub(super) fn en_instance(&self) {
        let dpid = self.dpid();
        let before = self.icon_desc(&dpid);
        let [ix, iy, iw, ih] = self
            .item_geom(&dpid)
            .unwrap_or_else(|| self.fail(&self.no_status_item()));
        let icon = format!("{},{iy},{},{ih}", ix - 40, iw + 80);
        self.shot_rect("en-7a-icon-before", &icon);
        self.choose_menu_item(&dpid, "Stop default", "en-7b-menu-stop");
        let stopped = self.wait(120, || self.state_default_is("Stopped", "Unavailable"));
        self.need(stopped, || {
            format!(
                "default が止まったことが状態ファイルに出ません: {}",
                self.show_state()
            )
        });
        let idle = self.wait(10, || {
            self.icon_desc(&dpid) == "wharfkeep: no instance running"
        });
        self.need(idle, || {
            format!(
                "アイコンが実行中なしになりません: {}",
                self.icon_desc(&dpid)
            )
        });
        self.shot_rect("en-7c-icon-stopped", &icon);
        self.say(&format!(
            "(7) 止めた: {} / アイコン「{}」（前は「{before}」）",
            self.show_state(),
            self.icon_desc(&dpid)
        ));
        self.choose_menu_item(&dpid, "Start default", "en-7d-menu-start");
        let started = self.wait(240, || self.state_default_is("Running", "Ready"));
        self.need(started, || {
            format!(
                "default が起動したことが状態ファイルに出ません: {}",
                self.show_state()
            )
        });
        let back = self.wait(10, || self.icon_desc(&dpid) == before);
        self.need(back, || {
            format!(
                "アイコンが「{before}」に戻りません: {}",
                self.icon_desc(&dpid)
            )
        });
        self.shot_rect("en-7e-icon-started", &icon);
        self.say(&format!(
            "(7) 起動した: {} / アイコン「{}」",
            self.show_state(),
            self.icon_desc(&dpid)
        ));
        self.choose_menu_item(&dpid, "Quit", "en-8-menu-quit");
        let quit = self.wait(10, || self.daemons_are(0));
        self.need(quit, || "「Quit」でデーモンが終わりません".into());
        self.say("(8) 「Quit」でデーモンが終わった");
    }

    /// ログイン時と同じく、launchdに読み込ませると起動し、異常終了すると起動し直し、終了では起動し直さない。
    pub(super) fn autostart(&self) {
        self.say("== autostart");
        let _ = self.launchctl(&["bootout", &self.service]);
        let plist = self.plist.to_string_lossy();
        let _ = self.launchctl(&["bootstrap", &self.domain, &plist]);
        let runs = self.wait(10, || self.launchd_runs());
        self.need(runs, || "bootstrapでデーモンが起動しません".into());
        let lpid = self.launchd_pid();
        self.say(&format!(
            "(autostart-2) bootstrapでデーモン pid={} が起動した",
            show_pid(lpid)
        ));
        // launchdは10秒より短い間隔では起こし直さない。
        let _ = ok(Command::new("kill").args(["-9", &show_pid(lpid)]));
        let again = self.wait(20, || self.launchd_restarted(lpid));
        self.need(again, || "kill -9 の後に起動し直しません".into());
        let lpid2 = show_pid(self.launchd_pid());
        self.say(&format!(
            "(autostart-3) kill -9 の後、pid={lpid2} で起動し直した"
        ));
        // launchdのデーモンは言語を選ばないので最後の項目で選ぶ。
        let seen = self.wait(10, || self.status_item_visible(&lpid2));
        self.need(seen, || {
            "launchdのデーモンのメニューバー項目がAXに見えません".into()
        });
        self.choose_menu_item(&lpid2, "last", "autostart-4-menu-quit");
        let quit = self.wait(10, || self.daemons_are(0));
        self.need(quit, || "終了でデーモンが終わりません".into());
        let restarted = self.wait(15, || self.daemons_are(1));
        self.need(!restarted, || "終了で止めたのに起動し直しました".into());
        self.need(self.print().contains("last exit code = 0"), || {
            "終了コードが0ではありません".into()
        });
        self.say("(autostart-4) 終了の後、15秒待っても起動し直さない（last exit code = 0）");
    }

    /// (5)(6) 自動起動をオフにすると、plistが消え、launchdは起こし直さない。(7) オンに戻すと登録される。
    pub(super) fn autostart_off_and_on(&self) {
        let _ = self.launchctl(&["kickstart", &self.service]);
        let runs = self.wait(10, || self.launchd_runs());
        self.need(runs, || "launchdからデーモンを起動できません".into());
        let opid = show_pid(self.launchd_pid());
        let seen = self.wait(10, || self.status_item_visible(&opid));
        self.need(seen, || {
            "launchdのデーモンのメニューバー項目がAXに見えません".into()
        });
        self.choose_menu_item(&opid, "login", "autostart-5-menu-off");
        let removed = self.wait(10, || !self.plist.exists());
        self.need(removed, || "オフにしてもplistが消えません".into());
        self.need(self.dpid() == opid, || {
            "オフにしたらデーモンが止まりました".into()
        });
        let still = self.wait(5, || self.status_item_visible(&opid));
        self.need(still, || {
            "オフにしたあとメニューバー項目が消えました".into()
        });
        self.say(&format!(
            "(autostart-5) オフにするとplistが消え、デーモン pid={opid} とメニューバーは残った"
        ));
        let _ = ok(Command::new("kill").args(["-9", &opid]));
        let gone = self.wait(10, || self.daemons_are(0));
        self.need(gone, || "kill -9 のあとデーモンが残っています".into());
        let restarted = self.wait(15, || self.daemons_are(1));
        self.need(!restarted, || {
            "オフにしたのにkill -9 のあと起こし直されました".into()
        });
        self.need(!self.loaded(), || {
            "オフにしたのにlaunchdにジョブが残っています".into()
        });
        self.say("(autostart-6) オフのあと kill -9 しても15秒間起こし直されず、launchdからジョブが外れた");
        let _ = Command::new(&self.exe)
            .arg("daemon")
            .env("WHARFKEEP_LANG", "en")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let up = self.wait(10, || self.daemons_are(1));
        self.need(up, || "デーモンが起動しません".into());
        let mpid = self.dpid();
        let seen = self.wait(10, || self.status_item_visible(&mpid));
        self.need(seen, || "メニューバー項目がAXに見えません".into());
        self.choose_menu_item(&mpid, "login", "autostart-7-menu-on");
        let loaded = self.wait(10, || self.loaded());
        self.need(loaded, || "オンに戻してもlaunchdに登録されません".into());
        self.need(self.plist.exists(), || {
            "オンに戻してもplistがありません".into()
        });
        self.say("(autostart-7) オンに戻すとplistが書かれ、launchdに登録された");
        self.choose_menu_item(&mpid, "last", "autostart-7-menu-quit");
        let quit = self.wait(10, || self.daemons_are(0));
        self.need(quit, || {
            "終了でデーモンが終わりません（オンに戻したあと）".into()
        });
    }
}
