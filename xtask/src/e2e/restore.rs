//! 後片付け。利用者の環境を試験の前の形に戻す。
use super::*;

impl Ctx {
    /// 試験の後は利用者の環境に合わせ、自動起動を有効のままlaunchdのデーモンを動かしておく。
    /// 真ならログイン時の起動先を戻せた。見張りと失敗が重なったときは、先の片付けが終わるまで待って同じ結果を返す。
    /// 片付けの中からfailを呼ぶと同じスレッドでロックを取り直して止まるので、ここから先ではfailを呼ばない。
    pub(super) fn restore(&self) -> bool {
        let mut result = self.restored.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = *result {
            return r;
        }
        self.restore_daemon();
        self.restore_login_item();
        self.restore_colima();
        self.restore_row();
        self.restore_iso();
        let r = self.back_to_applications();
        *result = Some(r);
        r
    }

    pub(super) fn restore_daemon(&self) {
        let _ = ok(Command::new("pkill").args(["-fx", &format!("{} gui", self.exe_re)]));
        let keep = self.launchd_pid().map(|p| p.to_string());
        self.daemon_pids()
            .lines()
            .filter(|p| Some(p.to_string()) != keep)
            .for_each(|p| {
                let _ = ok(Command::new("kill").arg(p));
            });
    }

    /// 自動起動をオフにした段階のあとで失敗すると、plistが消えたまま残る。設定を既定（オン）に戻し、
    /// 起動したデーモンにplistを書かせてから止め、launchdに任せる。
    pub(super) fn restore_login_item(&self) {
        self.write_missing_plist();
        if self.plist.exists() {
            self.load_and_start();
        }
    }

    pub(super) fn write_missing_plist(&self) {
        if self.plist.exists() {
            return;
        }
        let _ = fs::remove_file(self.data.join("settings.json"));
        let child = Command::new(&self.exe)
            .arg("daemon")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        self.wait(10, || self.plist.exists());
        child.into_iter().for_each(|c| reap(c, Duration::ZERO));
    }

    pub(super) fn load_and_start(&self) {
        if !self.loaded() {
            let plist = self.plist.to_string_lossy();
            let _ = self.launchctl(&["bootstrap", &self.domain, &plist]);
        }
        if self.launchd_pid().is_none() {
            let _ = self.launchctl(&["kickstart", &self.service]);
        }
    }

    pub(super) fn restore_colima(&self) {
        if self.colima_was_running && !colima_running() {
            let _ = ok(Command::new("colima").args(["start", "default"]));
        }
    }

    pub(super) fn restore_row(&self) {
        if colima_running() {
            self.row_cleanup();
        }
    }

    /// 専用のインスタンスと、その中のpullしたイメージと実行したコンテナを消す。
    pub(super) fn restore_iso(&self) {
        if !self.iso_created.load(Ordering::SeqCst) {
            return;
        }
        let has_sock = self
            .iso_sock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        if has_sock {
            let _ = ok(&mut self.dki(&["rm", "-f", WEB]));
            let _ = ok(&mut self.dki(&["rmi", "-f", WEB_IMAGE]));
        }
        let _ = ok(Command::new("colima").args(["delete", ISO, "--force", "--data"]));
    }

    /// /Applications/wharfkeep.appがあれば、試験のデーモンを止めてそちらを一度起動し、launchdへ渡して起動先を戻す。
    pub(super) fn back_to_applications(&self) -> bool {
        let app = Path::new(APPLICATIONS_APP);
        if !app.exists() {
            return true;
        }
        let _ = self.launchctl(&["bootout", &self.service]);
        let _ = ok(Command::new("pkill").args(["-fx", &format!("{}( daemon)?", self.exe_re)]));
        self.wait(10, || self.daemons_are(0));
        let _ = ok(Command::new("open").arg(app));
        // installと同じく、開いた.appのデーモンをlaunchdへ渡し、runningと起動先を確かめる。
        let handed = Setup::system().and_then(|s| hand_over(&s));
        if let Err(e) = &handed {
            eprintln!("e2e: 失敗: ログイン時の起動を {APPLICATIONS_APP} へ戻せません: {e}");
        }
        handed.is_ok()
    }
}
