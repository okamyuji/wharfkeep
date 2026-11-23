//! 専用のインスタンスでの削除と片付け、pullから実行と削除まで、日本語の画面の段。
use super::*;

impl Ctx {
    /// 削除と片付けの試験。専用のインスタンスの中で、タグ、ボリューム、ネットワーク、ビルドキャッシュを作る。
    pub(super) fn resources_setup(&self) {
        self.say("== resources");
        self.iso_create();
        self.iso_fill();
        self.iso_watch();
    }

    pub(super) fn iso_create(&self) {
        // 一覧を取れないときに「無い」と見なすと、利用者の同名インスタンスを最後にデータごと消してしまう。
        let list = text(Command::new("colima").args(["list", "--json"]))
            .unwrap_or_else(|| self.fail("colima list に失敗しました"));
        let exists = jq_input(&["-es", "--arg", "n", ISO, "any(.[]; .name == $n)"], &list);
        self.need(!exists, || {
            format!("colimaのインスタンス {ISO} が既にあります。試験が消すので、使っていないことを確かめてから colima delete {ISO} してください")
        });
        self.iso_created.store(true, Ordering::SeqCst);
        let started = ok(Command::new("colima").args([
            "start",
            ISO,
            "--cpu",
            "1",
            "--memory",
            "2",
            "--disk",
            "10",
            "--vm-type",
            "vz",
        ]));
        self.need(started, || format!("colima start {ISO} に失敗しました"));
        let sock = socket_of(ISO).unwrap_or_else(|| {
            self.fail(&format!("{ISO} のソケットを colima status から取れません"))
        });
        *self.iso_sock.lock().unwrap_or_else(|e| e.into_inner()) = Some(sock);
    }

    /// タグ、ボリューム、ネットワーク、ビルドキャッシュ、見本のコンテナを作る。
    pub(super) fn iso_fill(&self) {
        let pulled = ok(&mut self.dki(&["pull", "-q", "alpine:latest"]));
        self.need(pulled, || {
            format!("{ISO} に alpine:latest を取得できません")
        });
        [
            &["tag", "alpine:latest", "wk-e2e-img:del"][..],
            &["volume", "create", "wk-e2e-vol"],
            &["volume", "create"],
            &["network", "create", "wk e2e net"],
            &["network", "create", "wk-e2e-unused"],
        ]
        .into_iter()
        .for_each(|args| {
            let _ = ok(&mut self.dki(args));
        });
        // タグなしでビルドするとdanglingのイメージになる。層はビルドキャッシュに残る。
        let _ = piped(
            &mut self.dki(&["build", "-q", "-"]),
            "FROM alpine:latest\nRUN echo one > /x\n",
        );
        let dangling = self.dki_text(&["images", "-f", "dangling=true", "-q"]);
        self.need(!dangling.is_empty(), || {
            "dangling のイメージを作れません".into()
        });
        // コンテナ画面の見本。実行中、一時停止、停止（0で終了）、失敗（0以外で終了）を1つずつ置く。
        // READMEの画像はこの画面を使う。利用者のdefaultのコンテナ名を写さないため。
        [
            &[
                "run",
                "-d",
                "--name",
                "web",
                "alpine:latest",
                "sleep",
                "3600",
            ][..],
            &[
                "run",
                "-d",
                "--name",
                "cache",
                "alpine:latest",
                "sleep",
                "3600",
            ],
            &["pause", "cache"],
            &["run", "--name", "worker", "alpine:latest", "true"],
            &[
                "run",
                "--name",
                "job",
                "alpine:latest",
                "sh",
                "-c",
                "exit 3",
            ],
        ]
        .into_iter()
        .for_each(|args| {
            let _ = ok(&mut self.dki(args));
        });
    }

    pub(super) fn iso_watch(&self) {
        let _ = self.launchctl(&["kickstart", &self.service]);
        let runs = self.wait(10, || self.launchd_runs());
        self.need(runs, || {
            "launchdからデーモンを起動できません（resources）".into()
        });
        let ready = self.wait(90, || self.iso_ready());
        self.need(ready, || {
            format!("状態ファイルに {ISO} の Ready が出ません")
        });
    }

    /// WHARFKEEP_PROFILEで専用のインスタンスを選んだGUIを立てる。
    pub(super) fn open_iso_gui(&self, lang: &str) -> String {
        let _ = Command::new(&self.exe)
            .arg("gui")
            .env("WHARFKEEP_LANG", lang)
            .env("WHARFKEEP_PROFILE", ISO)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let up = self.wait(10, || self.guis_are(1));
        self.need(up, || format!("GUIのプロセスが立ちません（{ISO} {lang}）"));
        self.gpid()
    }

    pub(super) fn resources_samples(&self) {
        let gpid = self.open_iso_gui("en");
        self.see(
            20,
            &gpid,
            "checkbox Containers",
            "サイドバーの「Containers」がAXで見えません（resources）",
        );
        let _ = self.ax_press(&gpid, "checkbox", "Containers");
        self.see(
            15,
            &gpid,
            "button Stop web",
            "見本の行「Stop web」がAXで見えません",
        );
        self.see(
            15,
            &gpid,
            "button Start job",
            "見本の行「Start job」がAXで見えません",
        );
        let contents = self.contents(&gpid);
        let missing: Vec<&str> = ["Running", "Paused", "Stopped", "Failed"]
            .into_iter()
            .filter(|s| !judge::has_element(&contents, &format!("static text {s}")))
            .collect();
        self.need(missing.is_empty(), || {
            format!("見本の行に状態の文字 {missing:?} がありません")
        });
        self.shot_window("en-9-containers", &gpid);
        self.say("(9) 見本のコンテナ4つが、実行中・一時停止・停止・失敗の状態で並んだ");
        self.press(&gpid, "checkbox", "Dashboard");
        self.see(
            15,
            &gpid,
            "static text Failed: 1",
            "見本のダッシュボードに「Failed: 1」が出ません",
        );
        self.shot_window("en-9-dashboard", &gpid);
    }

    /// 確認ダイアログを開いて取り消し、もう一度開いて削除する。
    pub(super) fn delete_via_row(
        &self,
        gpid: &str,
        button: &str,
        exists: impl Fn() -> bool,
        shot: &str,
    ) {
        self.see(
            20,
            gpid,
            &format!("button {button}"),
            &format!("「{button}」がAXで見えません"),
        );
        self.press(gpid, "button", button);
        self.see(
            10,
            gpid,
            "button Cancel",
            &format!("「{button}」の確認ダイアログが出ません"),
        );
        self.shot_window(shot, gpid);
        self.press(gpid, "button", "Cancel");
        self.see(
            5,
            gpid,
            &format!("button {button}"),
            "取り消したあと行が見えません",
        );
        std::thread::sleep(Duration::from_secs(1));
        self.need(exists(), || {
            format!("「Cancel」で取り消したのに消えました: {button}")
        });
        self.press(gpid, "button", button);
        self.see(10, gpid, "button Cancel", "2度目の確認ダイアログが出ません");
        self.press(gpid, "button", "Delete");
        let gone = self.wait(60, || !exists());
        self.need(gone, || format!("「Delete」を押しても消えません: {button}"));
    }

    pub(super) fn resources_images(&self) {
        let gpid = self.gpid();
        // イメージ: 行のごみ箱でタグを消す（イメージ本体のalpineは残る）。
        self.see(
            20,
            &gpid,
            "checkbox Images",
            "サイドバーの「Images」がAXで見えません",
        );
        let _ = self.ax_press(&gpid, "checkbox", "Images");
        self.see(
            15,
            &gpid,
            "button Delete wk-e2e-img:del",
            "イメージの行がAXで見えません",
        );
        self.shot_window("en-9a-images", &gpid);
        let img = |r: &str| self.dki_ok(&["image", "inspect", r]);
        self.delete_via_row(
            &gpid,
            "Delete wk-e2e-img:del",
            || img("wk-e2e-img:del"),
            "en-9b-confirm-image",
        );
        self.need(img("alpine:latest"), || {
            "タグを消したら alpine:latest まで消えました".into()
        });
        // 本体のalpineは残るので、空いた容量ではなくタグを外した知らせが出る。
        self.see(
            15,
            &gpid,
            "static text Removed tag wk-e2e-img:del",
            "タグを外した通知が出ません",
        );
        self.say("(9) イメージの行のごみ箱: 取り消しで残り、削除でタグ wk-e2e-img:del が消え、その通知が出た");
        // イメージ: 「片付け…」でdanglingとビルドキャッシュを消す。
        self.press(&gpid, "button", "Clean up…");
        self.see(
            10,
            &gpid,
            "radio button Remove dangling images only",
            "片付けのダイアログが出ません",
        );
        let cache = judge::elem_name(&self.contents(&gpid), "checkbox", "Remove the build cache")
            .unwrap_or_else(|| self.fail("ビルドキャッシュの選択肢がありません"));
        self.press(&gpid, "radio button", "Remove dangling images only");
        self.press(&gpid, "checkbox", &cache);
        self.shot_window("en-9c-cleanup-images", &gpid);
        self.press(&gpid, "button", "Delete");
        let clean = self.wait(90, || {
            self.dki_text(&["images", "-f", "dangling=true", "-q"])
                .is_empty()
        });
        self.need(clean, || {
            "片付けのあとも dangling のイメージが残ります".into()
        });
        let empty = self.wait(60, || {
            judge::cache_empty(&self.dki_text(&["system", "df", "--format", "{{.Type}}|{{.Size}}"]))
        });
        self.need(empty, || {
            format!(
                "片付けのあともビルドキャッシュが残ります: {}",
                self.dki_text(&["system", "df"])
            )
        });
        let freed = self.wait(15, || self.has_prefix(&gpid, "static text Freed "));
        self.need(freed, || "空いた容量の通知が出ません".into());
        self.shot_window("en-9d-freed-toast", &gpid);
        self.say(
            "(9) イメージの「片付け…」: dangling とビルドキャッシュが消え、空いた容量の通知が出た",
        );
    }

    pub(super) fn resources_volumes_networks(&self) {
        let gpid = self.gpid();
        self.press(&gpid, "checkbox", "Volumes");
        self.see(
            15,
            &gpid,
            "button Delete wk-e2e-vol",
            "ボリュームの行がAXで見えません",
        );
        self.shot_window("en-9e-volumes", &gpid);
        let vol = || self.dki_ok(&["volume", "inspect", "wk-e2e-vol"]);
        self.delete_via_row(&gpid, "Delete wk-e2e-vol", vol, "en-9f-confirm-volume");
        self.press(&gpid, "button", "Clean up…");
        let asked = self.wait(10, || {
            self.has_prefix(&gpid, "static text Delete 1 unused anonymous volumes")
        });
        self.need(asked, || {
            "未使用ボリュームの件数が確認ダイアログに出ません".into()
        });
        self.shot_window("en-9g-cleanup-volumes", &gpid);
        self.press(&gpid, "button", "Delete");
        let none = self.wait(60, || self.dki_text(&["volume", "ls", "-q"]).is_empty());
        self.need(none, || {
            format!(
                "片付けのあともボリュームが残ります: {}",
                self.dki_text(&["volume", "ls", "-q"])
            )
        });
        self.say("(9) ボリュームの行のごみ箱と「片付け…」で wk-e2e-vol と無名ボリュームが消えた");
        // ネットワーク。名前に空白があるので、IDで消せることも確かめる。
        self.press(&gpid, "checkbox", "Networks");
        self.see(
            15,
            &gpid,
            "button Delete wk e2e net",
            "ネットワークの行がAXで見えません",
        );
        self.need(!self.has_element(&gpid, "button Delete bridge"), || {
            "既定の bridge に削除ボタンがあります".into()
        });
        self.press(&gpid, "button", "Inspect bridge");
        let inspected = self.wait(10, || self.has_prefix(&gpid, "static text Inspect: bridge"));
        self.need(inspected, || "詳細の見出しが出ません".into());
        self.shot_window("en-9h-networks", &gpid);
        let net = |n: &str| self.dki_ok(&["network", "inspect", n]);
        self.delete_via_row(
            &gpid,
            "Delete wk e2e net",
            || net("wk e2e net"),
            "en-9i-confirm-network",
        );
        self.press(&gpid, "button", "Clean up…");
        let asked = self.wait(10, || {
            self.has_prefix(&gpid, "static text Delete all unused networks")
        });
        self.need(asked, || "ネットワークの片付けの確認が出ません".into());
        self.press(&gpid, "button", "Delete");
        let gone = self.wait(60, || !net("wk-e2e-unused"));
        self.need(gone, || "片付けのあとも wk-e2e-unused が残ります".into());
        self.need(net("bridge"), || "既定の bridge が消えました".into());
        self.say(
            "(9) ネットワークの行のごみ箱（空白入りの名前）と「片付け…」で消え、bridge は残った",
        );
        self.close_window(&gpid);
    }

    /// 日本語のイメージ画面を1枚撮る。
    pub(super) fn resources_ja(&self) {
        let _ = ok(&mut self.dki(&["tag", "alpine:latest", "wk-e2e-ja:1"]));
        let filter = r#".docker[$n].Ready.images[]? | select(.repo_tags | index("wk-e2e-ja:1"))"#;
        let listed = self.wait(30, || self.jq_ok(&["--arg", "n", ISO, filter]));
        self.need(listed, || "状態ファイルに wk-e2e-ja:1 が出ません".into());
        let gpid = self.open_iso_gui("ja");
        self.see(
            20,
            &gpid,
            "checkbox イメージ",
            "サイドバーの「イメージ」がAXで見えません",
        );
        let _ = self.ax_press(&gpid, "checkbox", "イメージ");
        self.see(
            15,
            &gpid,
            "button 削除 wk-e2e-ja:1",
            "日本語のイメージの行がAXで見えません",
        );
        self.shot_window("ja-9-images", &gpid);
        self.close_window(&gpid);
        self.say("(9) 日本語のイメージ画面を撮った");
    }

    /// イメージ画面でpullし、コンテナ画面でポートを付けて実行し、ログとシェルを開き、確認を経て消す。
    pub(super) fn web(&self) {
        self.say("== web");
        let gpid = self.open_iso_gui("en");
        self.see(
            20,
            &gpid,
            "checkbox Images",
            "サイドバーの「Images」がAXで見えません（web）",
        );
        self.press(&gpid, "checkbox", "Images");
        self.see(
            15,
            &gpid,
            "button Pull",
            "イメージ画面に「Pull」がありません",
        );
        self.need(self.ax_set(&gpid, "Pull an image", WEB_IMAGE), || {
            "pull の入力欄に書けません".into()
        });
        self.press(&gpid, "button", "Pull");
        let pulled = self.wait(300, || self.dki_ok(&["image", "inspect", WEB_IMAGE]));
        self.need(pulled, || format!("GUIから {WEB_IMAGE} を pull できません"));
        self.see(
            30,
            &gpid,
            &format!("button Delete {WEB_IMAGE}"),
            &format!("イメージの行に {WEB_IMAGE} が出ません"),
        );
        self.shot_window("en-10a-pulled", &gpid);
        self.say(&format!("(10) イメージ画面で {WEB_IMAGE} を pull した"));
        self.web_run(&gpid);
        self.web_logs_and_shell(&gpid);
        let exists = || self.dki_ok(&["container", "inspect", WEB]);
        self.delete_via_row(
            &gpid,
            &format!("Delete {WEB}"),
            exists,
            "en-10d-confirm-container",
        );
        self.say(&format!("(10) 確認ダイアログを経て {WEB} を消した"));
        self.close_window(&gpid);
        self.need(self.daemons_are(1), || {
            "ウィンドウを閉じたらデーモンが残っていません".into()
        });
        self.say("(10) ウィンドウを閉じるとGUIは終わり、デーモンは残った");
    }

    pub(super) fn web_run(&self, gpid: &str) {
        self.press(gpid, "checkbox", "Containers");
        self.see(
            15,
            gpid,
            "UI element Run a container",
            "「Run a container」がありません",
        );
        self.press(gpid, "UI element", "Run a container");
        self.see(
            10,
            gpid,
            "button Run",
            "「Run a container」を開いても「Run」が出ません",
        );
        for (field, value) in [
            ("Image", WEB_IMAGE),
            ("Name", WEB),
            ("Ports (for example 8080:80, 5353:53/udp)", "8080:80"),
        ] {
            self.need(self.ax_set(gpid, field, value), || {
                format!("入力欄「{field}」に書けません")
            });
        }
        self.shot_window("en-10b-run-form", gpid);
        self.press(gpid, "button", "Run");
        let running = self.wait(60, || {
            self.dki_text(&["container", "inspect", "-f", "{{.State.Status}}", WEB]) == "running"
        });
        self.need(running, || format!("「Run」で {WEB} が動きません"));
        let url = format!("localhost:{WEB_PORT}");
        let served = self.wait(60, || {
            stdout(Command::new("curl").args(["-s", "-o", "/dev/null", "-w", "%{http_code}", &url]))
                == "200"
        });
        self.need(served, || format!("curl {url} が 200 を返しません"));
        self.say(&format!(
            "(10) コンテナ画面で {WEB} を 8080:80 で実行し、curl が 200 を返した"
        ));
    }

    pub(super) fn web_logs_and_shell(&self, gpid: &str) {
        self.press(gpid, "button", &format!("Logs {WEB}"));
        self.see(15, gpid, "checkbox Follow", "ログの「Follow」が出ません");
        let logged = self.wait(30, || self.contents(gpid).contains("GET / HTTP/1.1"));
        self.need(logged, || "ログに curl のアクセスが出ません".into());
        self.shot_window("en-10c-logs", gpid);
        self.press(gpid, "button", "Close");
        self.say("(10) ログを開き、curl のアクセスの行が出た");
        let id = self.dki_text(&["container", "inspect", "-f", "{{.Id}}", WEB]);
        let pattern = judge::shell_pattern(&id);
        self.press(gpid, "button", &format!("Shell {WEB}"));
        let shell = || first(&stdout(Command::new("pgrep").args(["-f", &pattern])));
        let opened = self.wait(30, || !shell().is_empty());
        self.need(opened, || {
            "外部ターミナルで docker exec が動きません".into()
        });
        let pid = shell();
        self.say(&format!(
            "(10) シェルを開き、外部ターミナルの docker exec pid={pid} を見つけた"
        ));
        self.close_terminal(&pid);
    }

    /// docker execを止めてから、そのttyのTerminal.appのウィンドウを閉じる。止める前に閉じると確認が出る。
    pub(super) fn close_terminal(&self, pid: &str) {
        let tty = judge::tty_path(&stdout(Command::new("ps").args(["-o", "tty=", "-p", pid])));
        let _ = ok(Command::new("kill").arg(pid));
        let ended = self.wait(10, || !ok(Command::new("kill").args(["-0", pid])));
        self.need(ended, || format!("docker exec pid={pid} が終わりません"));
        let Some(tty) = tty else {
            return eprintln!(
                "e2e: 警告: docker exec に端末が無いので、ターミナルのウィンドウは閉じません"
            );
        };
        let script = format!(
            "if application \"Terminal\" is running then
tell application \"Terminal\"
repeat with w in (every window)
if (tty of every tab of w) contains \"{tty}\" then close w
end repeat
end tell
end if"
        );
        let _ = self.osa(&[&script]);
        self.say(&format!("(10) {tty} のターミナルを閉じた"));
    }

    /// 日本語の文言が描けることを確かめる。メニューバーの流れは上で確かめたのでGUIだけを開く。
    pub(super) fn ja(&self) {
        self.say("== ja");
        self.need(colima_running(), || {
            "日本語の確かめの前にcolimaが動いていません".into()
        });
        // 行を出すにはデーモンが状態ファイルを書く必要がある。
        let _ = self.launchctl(&["kickstart", &self.service]);
        let runs = self.wait(10, || self.launchd_runs());
        self.need(runs, || "launchdからデーモンを起動できません（ja）".into());
        self.row_up();
        let _ = Command::new(&self.exe)
            .arg("gui")
            .env("WHARFKEEP_LANG", "ja")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let up = self.wait(10, || self.guis_are(1));
        self.need(up, || "GUIのプロセスが立ちません（ja）".into());
        let gpid = self.gpid();
        self.check_navigation(&gpid, "コンテナ", "ダッシュボード");
        self.see(
            15,
            &gpid,
            &format!("button 停止 {ROW}"),
            &format!("日本語の行の「停止 {ROW}」がAXで見えません"),
        );
        self.need(self.has_element(&gpid, "static text 実行中"), || {
            "日本語の行に状態の文字「実行中」がありません".into()
        });
        self.shot_window("ja-5-gui", &gpid);
        self.close_window(&gpid);
        self.row_cleanup();
        self.say("(ja) 「コンテナ」を押し、見出しが「コンテナ」になり、閉じるとGUIが終わった");
    }

    pub(super) fn finish(&self) {
        let _ = self.launchctl(&["kickstart", &self.service]);
        let runs = self.wait(10, || self.launchd_runs());
        self.need(runs, || "launchdからデーモンを起動し直せません".into());
        self.need(is_running(&self.print()), || {
            "終わりにlaunchdのデーモンが running ではありません".into()
        });
        self.need(self.plist.exists(), || "終わりにplistがありません".into());
        self.say(&format!(
            "(restore) launchdのデーモン pid={} が running で、plistが残ったまま終える",
            show_pid(self.launchd_pid())
        ));
    }
}
