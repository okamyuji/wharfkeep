//! デーモン、GUI、launchd、状態ファイル、dockerの今の様子を読む。
use super::*;

impl Ctx {
    pub(super) fn pids(&self, suffix: &str) -> String {
        let pattern = format!("{}{suffix}", self.exe_re);
        stdout(Command::new("pgrep").args(["-fx", &pattern]))
    }

    pub(super) fn daemon_pids(&self) -> String {
        self.pids("( daemon)?")
    }

    pub(super) fn gui_pids(&self) -> String {
        self.pids(" gui")
    }

    pub(super) fn daemons_are(&self, n: usize) -> bool {
        judge::count_lines(&self.daemon_pids()) == n
    }

    pub(super) fn guis_are(&self, n: usize) -> bool {
        judge::count_lines(&self.gui_pids()) == n
    }

    pub(super) fn print(&self) -> String {
        stdout(Command::new("launchctl").args(["print", &self.service]))
    }

    pub(super) fn loaded(&self) -> bool {
        ok(Command::new("launchctl").args(["print", &self.service]))
    }

    pub(super) fn launchd_pid(&self) -> Option<u32> {
        judge::launchd_pid(&self.print())
    }

    pub(super) fn launchd_runs(&self) -> bool {
        self.launchd_pid().is_some() && self.daemons_are(1)
    }

    pub(super) fn launchd_restarted(&self, before: Option<u32>) -> bool {
        let now = self.launchd_pid();
        now.is_some() && now != before && self.daemons_are(1)
    }

    pub(super) fn launchctl(&self, args: &[&str]) -> bool {
        ok(Command::new("launchctl").args(args))
    }

    // ---- 状態ファイル（jq） ----

    pub(super) fn jq_ok(&self, args: &[&str]) -> bool {
        ok(Command::new("jq").arg("-e").args(args).arg(&self.state))
    }

    pub(super) fn jq_text(&self, args: &[&str]) -> String {
        stdout(Command::new("jq").args(args).arg(&self.state))
            .trim()
            .to_string()
    }

    pub(super) fn state_has_default(&self) -> bool {
        self.jq_ok(&[r#"any(.instances[]; .name == "default")"#])
    }

    /// 状態ファイルのdefault。例: `state_default_is("Running", "Ready")`。
    pub(super) fn state_default_is(&self, status: &str, docker: &str) -> bool {
        self.jq_ok(&[
            "--arg",
            "s",
            status,
            "--arg",
            "d",
            docker,
            r#"any(.instances[]; .name == "default" and .status == $s) and (.docker.default | has($d))"#,
        ])
    }

    pub(super) fn show_state(&self) -> String {
        self.jq_text(&[
            "-c",
            "{instances: [.instances[] | {name, status}], docker: (.docker | map_values(keys[0]))}",
        ])
    }

    pub(super) fn row_state(&self) -> String {
        self.jq_text(&[
            "-r",
            "--arg",
            "n",
            ROW,
            ".docker.default.Ready.containers[]? | select(.name == $n) | .state",
        ])
    }

    pub(super) fn row_is(&self, s: &str) -> bool {
        self.row_state() == s
    }

    pub(super) fn iso_ready(&self) -> bool {
        self.jq_text(&[
            "-r",
            "--arg",
            "n",
            ISO,
            r#".docker[$n] // {} | keys[0] // """#,
        ]) == "Ready"
    }

    // ---- docker ----

    /// ソケットが取れないときは、docker contextの先（利用者の環境）へ流さずに失敗させる。
    pub(super) fn docker(&self, sock: Option<String>, name: &str, args: &[&str]) -> Command {
        let sock = sock.unwrap_or_else(|| self.fail(&format!("{name} のソケットが取れません")));
        let mut cmd = Command::new("docker");
        cmd.args(args).env("DOCKER_HOST", sock);
        cmd
    }

    pub(super) fn dk(&self, args: &[&str]) -> Command {
        self.docker(self.default_sock.clone(), "default", args)
    }

    pub(super) fn dki(&self, args: &[&str]) -> Command {
        let sock = self
            .iso_sock
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        self.docker(sock, ISO, args)
    }

    pub(super) fn dki_ok(&self, args: &[&str]) -> bool {
        ok(&mut self.dki(args))
    }

    pub(super) fn dki_text(&self, args: &[&str]) -> String {
        stdout(&mut self.dki(args)).trim().to_string()
    }

    /// 後片付けからも呼ぶので、ソケットが無くても失敗にしない。
    pub(super) fn row_cleanup(&self) {
        if let Some(sock) = &self.default_sock {
            let rm = Command::new("docker")
                .args(["rm", "-f", ROW])
                .env("DOCKER_HOST", sock)
                .status();
            drop(rm);
        }
    }

    pub(super) fn row_up(&self) {
        self.row_cleanup();
        let made = ok(&mut self.dk(&["run", "-d", "--name", ROW, "alpine:latest", "sleep", "300"]));
        self.need(made, || {
            format!("alpine:latest でコンテナ {ROW} を作れません")
        });
        let up = self.wait(30, || self.row_is("running"));
        self.need(up, || format!("状態ファイルに実行中の {ROW} が出ません"));
    }
}
