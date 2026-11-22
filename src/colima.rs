use crate::i18n::{Lang, Msg};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// colima の操作で起きる失敗。文言は `message` で言語を選んで作る。
#[derive(Debug, Clone, PartialEq)]
pub enum ColimaError {
    Parse { line: usize, detail: String },
    InvalidSocket(String),
    InvalidProfile(String),
    InvalidCpus,
    InvalidMemory(f32),
    InvalidDisk,
    Spawn(String),
    Failed { code: Option<i32>, stderr: String },
    OutputTooLarge,
    Timeout,
}

impl ColimaError {
    pub fn message(&self, lang: Lang) -> String {
        let t = |m: Msg| m.text(lang);
        match self {
            Self::Parse { line, detail } => format!("{}: {line}: {detail}", t(Msg::ColimaParse)),
            Self::InvalidSocket(s) => format!("{}: {s}", t(Msg::ColimaSocket)),
            Self::InvalidProfile(s) => format!("{}: {s}", t(Msg::ColimaProfile)),
            Self::InvalidCpus => t(Msg::ColimaCpus).into(),
            Self::InvalidMemory(v) => format!("{}: {v}", t(Msg::ColimaMemory)),
            Self::InvalidDisk => t(Msg::ColimaDisk).into(),
            Self::Spawn(e) => format!("{}: {e}", t(Msg::ColimaSpawn)),
            Self::OutputTooLarge => t(Msg::ColimaOutputTooLarge).into(),
            Self::Timeout => t(Msg::ColimaTimeout).into(),
            Self::Failed { code, stderr } => {
                let code = code.map_or("-".to_string(), |c| c.to_string());
                format!("{}: {code}: {}", t(Msg::ColimaFailed), stderr.trim())
            }
        }
    }
}

impl std::fmt::Display for ColimaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message(Lang::En))
    }
}

impl std::error::Error for ColimaError {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum InstanceStatus {
    Running,
    Stopped,
    Other(String),
}

impl From<String> for InstanceStatus {
    fn from(s: String) -> Self {
        match s.as_str() {
            "Running" => Self::Running,
            "Stopped" => Self::Stopped,
            _ => Self::Other(s),
        }
    }
}

impl From<InstanceStatus> for String {
    fn from(s: InstanceStatus) -> Self {
        match s {
            InstanceStatus::Running => "Running".into(),
            InstanceStatus::Stopped => "Stopped".into(),
            InstanceStatus::Other(s) => s,
        }
    }
}

/// `colima list --json` の1行。キー名は colima の出力に合わせる。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    pub name: String,
    pub status: InstanceStatus,
    pub arch: String,
    pub cpus: u32,
    #[serde(rename = "memory")]
    pub memory_bytes: u64,
    #[serde(rename = "disk")]
    pub disk_bytes: u64,
    pub runtime: String,
}

/// `colima list --json` は1行に1つの JSON を出す。行番号は空行も数える。
pub fn parse_list(ndjson: &str) -> Result<Vec<Instance>, ColimaError> {
    ndjson
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
        .map(|(i, l)| {
            serde_json::from_str(l).map_err(|e| ColimaError::Parse {
                line: i + 1,
                detail: e.to_string(),
            })
        })
        .collect()
}

pub fn parse_docker_socket(status_json: &str) -> Result<PathBuf, ColimaError> {
    #[derive(Deserialize)]
    struct Status {
        docker_socket: String,
    }
    let st: Status = serde_json::from_str(status_json).map_err(|e| ColimaError::Parse {
        line: e.line(),
        detail: e.to_string(),
    })?;
    match st.docker_socket.strip_prefix("unix://") {
        Some(p) if p.starts_with('/') => Ok(PathBuf::from(p)),
        _ => Err(ColimaError::InvalidSocket(st.docker_socket)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VmType {
    Vz,
    Qemu,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Arch {
    Aarch64,
    X86_64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MountType {
    Virtiofs,
    Sshfs,
    NineP,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreateSpec {
    pub profile: String,
    pub cpus: u32,
    pub memory_gib: f32,
    pub disk_gib: u32,
    pub vm_type: VmType,
    pub arch: Arch,
    pub mount_type: MountType,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    Start,
    Stop,
    Restart,
    Delete,
}

/// プロフィール名は colima のデータ置き場のパス部品になるので、`.` と `..` も断る。
/// 先頭の `-` は colima がオプションとして解釈する（`-h`、`--force`）ので断る。
fn check_profile(p: &str) -> Result<(), ColimaError> {
    let ok = !p.is_empty()
        && !p.starts_with('-')
        && p.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !p.chars().all(|c| c == '.');
    ok.then_some(())
        .ok_or_else(|| ColimaError::InvalidProfile(p.into()))
}

pub fn start_args(spec: &CreateSpec) -> Result<Vec<String>, ColimaError> {
    check_profile(&spec.profile)?;
    if spec.cpus == 0 {
        return Err(ColimaError::InvalidCpus);
    }
    if !spec.memory_gib.is_finite() || spec.memory_gib <= 0.0 {
        return Err(ColimaError::InvalidMemory(spec.memory_gib));
    }
    if spec.disk_gib == 0 {
        return Err(ColimaError::InvalidDisk);
    }
    let vm = match spec.vm_type {
        VmType::Vz => "vz",
        VmType::Qemu => "qemu",
    };
    let arch = match spec.arch {
        Arch::Aarch64 => "aarch64",
        Arch::X86_64 => "x86_64",
    };
    let mount = match spec.mount_type {
        MountType::Virtiofs => "virtiofs",
        MountType::Sshfs => "sshfs",
        MountType::NineP => "9p",
    };
    Ok([
        "start",
        &spec.profile,
        "--cpus",
        &spec.cpus.to_string(),
        "--memory",
        &spec.memory_gib.to_string(),
        "--disk",
        &spec.disk_gib.to_string(),
        "--vm-type",
        vm,
        "--arch",
        arch,
        "--mount-type",
        mount,
        "--runtime",
        "docker",
    ]
    .map(String::from)
    .to_vec())
}

/// 削除の確認は GUI 側で行うので、colima には `--force` を渡す。
pub fn simple_args(action: Action, profile: &str) -> Result<Vec<String>, ColimaError> {
    check_profile(profile)?;
    let (verb, force) = match action {
        Action::Start => ("start", false),
        Action::Stop => ("stop", false),
        Action::Restart => ("restart", false),
        Action::Delete => ("delete", true),
    };
    let mut v = vec![verb.to_string(), profile.to_string()];
    if force {
        v.push("--force".into());
    }
    Ok(v)
}

/// `list` と `status` の待ち時間。`start` などの長い処理は `run_streaming` を通るので対象外。
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);
/// JSON を読むための stdout の上限。`colima list --json` は数 KiB なので十分に大きい。
const STDOUT_LIMIT: usize = 1024 * 1024;
/// 1行と stderr の保持量の上限。超えた分は読み捨てる。
const LINE_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct Colima {
    bin: PathBuf,
    /// 子に渡す`PATH`。
    path_env: OsString,
}

impl Colima {
    pub fn new(bin: PathBuf) -> Self {
        // launchdや`open`から起動するとPATHが`/usr/bin:/bin:/usr/sbin:/sbin`だけになり、
        // colimaが呼ぶlimactlやdockerが見つからない。それらはcolimaと同じ場所に入る。
        let inherited = std::env::var_os("PATH").unwrap_or_default();
        let dirs = bin
            .parent()
            .filter(|d| d.is_absolute())
            .map(Path::to_path_buf)
            .into_iter()
            .chain(std::env::split_paths(&inherited));
        let path_env = std::env::join_paths(dirs).unwrap_or(inherited);
        Self { bin, path_env }
    }

    pub async fn list(&self) -> Result<Vec<Instance>, ColimaError> {
        parse_list(&self.capture(&["list", "--json"]).await?)
    }

    pub async fn docker_socket(&self, profile: &str) -> Result<PathBuf, ColimaError> {
        check_profile(profile)?;
        parse_docker_socket(&self.capture(&["status", "--json", "-p", profile]).await?)
    }

    async fn capture(&self, args: &[&str]) -> Result<String, ColimaError> {
        self.capture_within(args, CAPTURE_TIMEOUT).await
    }

    /// 固まった colima で状態の更新が止まらないよう、`limit` で打ち切る。落とした future が子を kill する。
    async fn capture_within(&self, args: &[&str], limit: Duration) -> Result<String, ColimaError> {
        tokio::time::timeout(limit, self.capture_unbounded(args))
            .await
            .map_err(|_| ColimaError::Timeout)?
    }

    async fn capture_unbounded(&self, args: &[&str]) -> Result<String, ColimaError> {
        let mut child = Command::new(&self.bin)
            .args(args)
            .env("PATH", &self.path_env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| ColimaError::Spawn(e.to_string()))?;
        // piped を指定したので取れないことはない。
        let mut stdout = child.stdout.take().expect("piped");
        let mut stderr = child.stderr.take().expect("piped");
        // stderr を別タスクで読む。stdout だけ読むと、stderr のパイプが詰まった子が止まる。
        let err_task =
            tokio::spawn(async move { read_capped(&mut stderr, LINE_LIMIT, true).await });
        let out = read_capped(&mut stdout, STDOUT_LIMIT, false).await;
        let spawn_err = |e: std::io::Error| ColimaError::Spawn(e.to_string());
        let (out, truncated) = match out {
            Ok(v) => v,
            Err(e) => {
                err_task.abort();
                return Err(spawn_err(e));
            }
        };
        if truncated {
            // 読み捨てると終わらない出力で詰まるので、子を止めて打ち切る。
            let _ = child.kill().await;
            err_task.abort();
            return Err(ColimaError::OutputTooLarge);
        }
        let status = child.wait().await.map_err(spawn_err)?;
        let (err, _) = err_task
            .await
            .expect("stderr task does not panic")
            .map_err(spawn_err)?;
        if !status.success() {
            return Err(ColimaError::Failed {
                code: status.code(),
                stderr: String::from_utf8_lossy(&err).into_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    /// stdout と stderr の行を1本の受け口へ流す。終了状態は両方が閉じたあとに返る。
    pub fn run_streaming(
        &self,
        args: Vec<String>,
    ) -> (
        mpsc::Receiver<String>,
        JoinHandle<Result<ExitStatus, ColimaError>>,
    ) {
        let (tx, rx) = mpsc::channel(256);
        let bin = self.bin.clone();
        let path_env = self.path_env.clone();
        (rx, tokio::spawn(stream(bin, path_env, args, tx)))
    }
}

async fn stream(
    bin: PathBuf,
    path_env: OsString,
    args: Vec<String>,
    tx: mpsc::Sender<String>,
) -> Result<ExitStatus, ColimaError> {
    let mut child = Command::new(bin)
        .args(args)
        .env("PATH", path_env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| ColimaError::Spawn(e.to_string()))?;
    // piped を指定したので取れないことはない。
    let out = forward(child.stdout.take().expect("piped"), tx.clone());
    let err = forward(child.stderr.take().expect("piped"), tx);
    let status = child
        .wait()
        .await
        .map_err(|e| ColimaError::Spawn(e.to_string()))?;
    let _ = tokio::join!(out, err);
    Ok(status)
}

fn forward<R: AsyncRead + Unpin + Send + 'static>(
    r: R,
    tx: mpsc::Sender<String>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut r = BufReader::new(r);
        // 受け手が消えても読み続ける。読むのをやめるとパイプが詰まって子が止まる。
        while let Ok(Some(l)) = next_line_capped(&mut r, LINE_LIMIT).await {
            let _ = tx.send(l).await;
        }
    })
}

/// 先頭 `cap` バイトを返す。2つ目の値は、`cap` を超える出力があったかどうか。
/// `drain` が真なら、超えた分を最後まで読み捨てる。偽なら超えた時点で戻る。
async fn read_capped<R: AsyncRead + Unpin>(
    r: &mut R,
    cap: usize,
    drain: bool,
) -> std::io::Result<(Vec<u8>, bool)> {
    let mut buf = Vec::new();
    (&mut *r).take(cap as u64 + 1).read_to_end(&mut buf).await?;
    let truncated = buf.len() > cap;
    buf.truncate(cap);
    if truncated && drain {
        // 子が書き終えるまでパイプを空にしないと、書き込み側が詰まる。
        tokio::io::copy(r, &mut tokio::io::sink()).await?;
    }
    Ok((buf, truncated))
}

/// `lines()` と同じく `\n` と `\r\n` で区切る。ただし1行の保持量は `cap` バイトまでで、超えた分は捨てる。
async fn next_line_capped<R: AsyncBufRead + Unpin>(
    r: &mut R,
    cap: usize,
) -> std::io::Result<Option<String>> {
    let mut line = Vec::new();
    let mut any = false;
    loop {
        let buf = r.fill_buf().await?;
        if buf.is_empty() {
            break;
        }
        any = true;
        let nl = buf.iter().position(|&b| b == b'\n');
        let end = nl.unwrap_or(buf.len());
        let room = cap.saturating_sub(line.len());
        line.extend_from_slice(&buf[..end.min(room)]);
        r.consume(end + usize::from(nl.is_some()));
        if nl.is_some() {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            break;
        }
    }
    Ok(any.then(|| String::from_utf8_lossy(&line).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const LIST: &str = include_str!("../tests/fixtures/colima_list.ndjson");
    const STATUS: &str = include_str!("../tests/fixtures/colima_status.json");

    #[test]
    fn parses_running_instance_from_real_output() {
        let v = parse_list(LIST).unwrap();
        assert_eq!(v[0].name, "default");
        assert_eq!(v[0].status, InstanceStatus::Running);
        assert_eq!(v[0].cpus, 4);
        assert_eq!(v[0].memory_bytes, 8589934592);
        assert_eq!(v[0].disk_bytes, 64424509440);
        assert_eq!(v[0].arch, "aarch64");
        assert_eq!(v[0].runtime, "docker");
    }

    #[test]
    fn parses_multiple_and_stopped() {
        let v = parse_list(LIST).unwrap();
        let s: Vec<_> = v
            .iter()
            .map(|i| (i.name.as_str(), i.status.clone()))
            .collect();
        assert_eq!(
            s,
            [
                ("default", InstanceStatus::Running),
                ("dev", InstanceStatus::Stopped),
                ("work", InstanceStatus::Running)
            ]
        );
    }

    #[test]
    fn unknown_status_is_kept() {
        let v = parse_list(r#"{"name":"a","status":"Broken","arch":"x","cpus":1,"memory":1,"disk":1,"runtime":"docker"}"#).unwrap();
        assert_eq!(v[0].status, InstanceStatus::Other("Broken".into()));
    }

    #[test]
    fn ignores_unknown_fields_and_blank_lines() {
        let noisy = format!(
            "\n  \n{}\n\n",
            LIST.trim_end()
                .replace("\"runtime\"", "\"extra\":[1,2],\"runtime\"")
        );
        assert_eq!(parse_list(&noisy).unwrap(), parse_list(LIST).unwrap());
        assert!(parse_list("").unwrap().is_empty());
    }

    #[test]
    fn rejects_broken_json_with_line_number() {
        let first = LIST.lines().next().unwrap();
        let input = format!("{first}\n\n{{broken\n{first}");
        let e = parse_list(&input).unwrap_err();
        assert!(matches!(e, ColimaError::Parse { line: 3, .. }), "{e:?}");
        assert!(e.message(Lang::En).contains('3'));
        assert!(parse_list(r#"{"name":"a"}"#).is_err());
    }

    #[test]
    fn extracts_socket_path() {
        assert_eq!(
            parse_docker_socket(STATUS).unwrap(),
            PathBuf::from("/Users/example/.colima/default/docker.sock")
        );
    }

    #[test]
    fn rejects_bad_socket() {
        for s in [
            "nope",
            r#"{"cpu":1}"#,
            r#"{"docker_socket":"tcp://x"}"#,
            r#"{"docker_socket":"unix://relative"}"#,
        ] {
            assert!(parse_docker_socket(s).is_err(), "{s}");
        }
    }

    fn spec() -> CreateSpec {
        CreateSpec {
            profile: "dev".into(),
            cpus: 1,
            memory_gib: 0.5,
            disk_gib: 20,
            vm_type: VmType::Vz,
            arch: Arch::Aarch64,
            mount_type: MountType::Virtiofs,
        }
    }

    #[test]
    fn start_args_contains_all_options() {
        let want = [
            "start",
            "dev",
            "--cpus",
            "1",
            "--memory",
            "0.5",
            "--disk",
            "20",
            "--vm-type",
            "vz",
            "--arch",
            "aarch64",
            "--mount-type",
            "virtiofs",
            "--runtime",
            "docker",
        ];
        assert_eq!(start_args(&spec()).unwrap(), want);
    }

    #[test]
    fn start_args_formats_whole_memory_and_other_variants() {
        let s = CreateSpec {
            memory_gib: 2.0,
            vm_type: VmType::Qemu,
            arch: Arch::X86_64,
            mount_type: MountType::Sshfs,
            ..spec()
        };
        let a = start_args(&s).unwrap();
        assert_eq!(&a[4..6], ["--memory", "2"]);
        assert_eq!(
            &a[8..14],
            [
                "--vm-type",
                "qemu",
                "--arch",
                "x86_64",
                "--mount-type",
                "sshfs"
            ]
        );
        let s = CreateSpec {
            mount_type: MountType::NineP,
            ..spec()
        };
        assert_eq!(start_args(&s).unwrap()[13], "9p");
    }

    #[test]
    fn start_args_rejects_empty_or_invalid_profile() {
        for p in [
            "", "a b", "../x", ".", "..", "a/b", "日本", "-h", "--force", "-",
        ] {
            let s = CreateSpec {
                profile: p.into(),
                ..spec()
            };
            assert!(
                matches!(start_args(&s), Err(ColimaError::InvalidProfile(_))),
                "{p:?}"
            );
        }
        for p in ["a", "A-z_0.9", "..a", "a..", "a-b"] {
            let s = CreateSpec {
                profile: p.into(),
                ..spec()
            };
            assert!(start_args(&s).is_ok(), "{p:?}");
        }
    }

    #[test]
    fn start_args_rejects_zero_cpu_and_memory() {
        let bad = [
            CreateSpec { cpus: 0, ..spec() },
            CreateSpec {
                memory_gib: 0.0,
                ..spec()
            },
            CreateSpec {
                memory_gib: -1.0,
                ..spec()
            },
            CreateSpec {
                memory_gib: f32::NAN,
                ..spec()
            },
            CreateSpec {
                memory_gib: f32::INFINITY,
                ..spec()
            },
            CreateSpec {
                disk_gib: 0,
                ..spec()
            },
        ];
        for s in bad {
            assert!(start_args(&s).is_err(), "{s:?}");
        }
        let min = CreateSpec {
            cpus: 1,
            memory_gib: f32::MIN_POSITIVE,
            disk_gib: 1,
            ..spec()
        };
        assert!(start_args(&min).is_ok());
    }

    #[test]
    fn simple_args_per_action() {
        assert_eq!(
            simple_args(Action::Delete, "dev").unwrap(),
            ["delete", "dev", "--force"]
        );
        assert_eq!(simple_args(Action::Start, "dev").unwrap(), ["start", "dev"]);
        assert_eq!(simple_args(Action::Stop, "dev").unwrap(), ["stop", "dev"]);
        assert_eq!(
            simple_args(Action::Restart, "dev").unwrap(),
            ["restart", "dev"]
        );
    }

    #[test]
    fn simple_args_rejects_option_like_and_invalid_profiles() {
        for p in ["-h", "--force", "", "a b", ".."] {
            assert!(
                matches!(
                    simple_args(Action::Delete, p),
                    Err(ColimaError::InvalidProfile(_))
                ),
                "{p:?}"
            );
        }
    }

    #[test]
    fn every_error_message_differs_by_language_and_has_its_data() {
        let cases = [
            (
                ColimaError::Parse {
                    line: 42,
                    detail: "d-x".into(),
                },
                "42",
            ),
            (ColimaError::InvalidSocket("sock-x".into()), "sock-x"),
            (ColimaError::InvalidProfile("prof-x".into()), "prof-x"),
            (ColimaError::InvalidCpus, ""),
            (ColimaError::InvalidMemory(7.5), "7.5"),
            (ColimaError::InvalidDisk, ""),
            (ColimaError::Spawn("io-x".into()), "io-x"),
            (ColimaError::OutputTooLarge, ""),
            (ColimaError::Timeout, ""),
            (
                ColimaError::Failed {
                    code: Some(9),
                    stderr: "err-x".into(),
                },
                "err-x",
            ),
            (
                ColimaError::Failed {
                    code: None,
                    stderr: String::new(),
                },
                "",
            ),
        ];
        for (e, data) in cases {
            let (en, ja) = (e.message(Lang::En), e.message(Lang::Ja));
            assert_ne!(en, ja, "{e:?}");
            assert!(en.contains(data) && ja.contains(data), "{e:?}");
            assert_eq!(e.to_string(), en);
        }
        let f = ColimaError::Failed {
            code: Some(9),
            stderr: String::new(),
        };
        assert!(f.message(Lang::En).contains('9'));
    }

    #[tokio::test]
    async fn run_streaming_merges_both_streams_and_returns_status() {
        let c = Colima::new(PathBuf::from("/bin/sh"));
        let args = ["-c", "echo out; echo err 1>&2; exit 3"]
            .map(String::from)
            .to_vec();
        let (mut rx, h) = c.run_streaming(args);
        let mut lines = vec![];
        while let Some(l) = rx.recv().await {
            lines.push(l);
        }
        lines.sort();
        assert_eq!(lines, ["err", "out"]);
        assert_eq!(h.await.unwrap().unwrap().code(), Some(3));
    }

    #[tokio::test]
    async fn run_streaming_reports_spawn_failure() {
        let c = Colima::new(PathBuf::from("/nonexistent/colima"));
        let (mut rx, h) = c.run_streaming(vec![]);
        assert!(rx.recv().await.is_none());
        assert!(matches!(h.await.unwrap(), Err(ColimaError::Spawn(_))));
    }

    #[tokio::test]
    async fn children_see_the_binary_dir_first_in_path() {
        let d = tempfile::tempdir().unwrap();
        let c = fake(&d, r#"echo "$PATH""#);
        let prefix = format!("{}:", d.path().display());
        let inherited = std::env::var("PATH").unwrap();
        let out = c.capture(&[]).await.unwrap();
        assert!(out.starts_with(&prefix), "{out}");
        assert!(out.trim_end().ends_with(&inherited), "{out}");
        let (mut rx, h) = c.run_streaming(vec![]);
        let line = rx.recv().await.unwrap();
        assert!(line.starts_with(&prefix), "{line}");
        assert!(h.await.unwrap().unwrap().success());
    }

    fn fake(dir: &tempfile::TempDir, body: &str) -> Colima {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.path().join("colima");
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        Colima::new(p)
    }

    #[tokio::test]
    async fn list_and_socket_run_the_binary_with_expected_args() {
        let d = tempfile::tempdir().unwrap();
        let c = fake(
            &d,
            r#"case "$*" in
"list --json") cat "$FIX" ;;
"status --json -p dev") echo '{"docker_socket":"unix:///s/d.sock"}' ;;
*) echo "bad args: $*" 1>&2; exit 1 ;;
esac"#
                .replace(
                    "$FIX",
                    &format!(
                        "{}/tests/fixtures/colima_list.ndjson",
                        env!("CARGO_MANIFEST_DIR")
                    ),
                )
                .as_str(),
        );
        assert_eq!(c.list().await.unwrap().len(), 3);
        assert_eq!(
            c.docker_socket("dev").await.unwrap(),
            PathBuf::from("/s/d.sock")
        );
        assert!(matches!(
            c.docker_socket("a b").await,
            Err(ColimaError::InvalidProfile(_))
        ));
    }

    #[tokio::test]
    async fn nonzero_exit_becomes_failed_with_stderr() {
        let d = tempfile::tempdir().unwrap();
        let c = fake(&d, "echo boom 1>&2; exit 5");
        match c.list().await {
            Err(ColimaError::Failed {
                code: Some(5),
                stderr,
            }) => assert_eq!(stderr.trim(), "boom"),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn capture_accepts_stdout_up_to_the_limit_and_rejects_beyond() {
        let d = tempfile::tempdir().unwrap();
        let at = fake(&d, "head -c 1048576 /dev/zero | tr '\\0' x");
        assert_eq!(at.capture(&[]).await.unwrap().len(), 1024 * 1024);
        let over = fake(&d, "head -c 1048577 /dev/zero | tr '\\0' x");
        assert_eq!(over.capture(&[]).await, Err(ColimaError::OutputTooLarge));
    }

    #[tokio::test]
    async fn capture_kills_a_hung_colima_after_the_limit() {
        let d = tempfile::tempdir().unwrap();
        let pid_file = d.path().join("pid");
        let c = fake(
            &d,
            &format!("echo $$ > '{}'; exec sleep 30", pid_file.display()),
        );
        let started = std::time::Instant::now();
        let r = c
            .capture_within(&[], std::time::Duration::from_millis(500))
            .await;
        assert_eq!(r, Err(ColimaError::Timeout));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        // 負荷でシェルの起動が遅いと、pid を書く前に kill される。kill しない実装なら 2 秒以内に必ず書く。
        let mut pid = None;
        for _ in 0..40 {
            if let Ok(p) = std::fs::read_to_string(&pid_file) {
                pid = Some(p);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let Some(pid) = pid else { return };
        let alive = || {
            std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .output()
                .unwrap()
                .status
                .success()
        };
        for _ in 0..40 {
            if !alive() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("colima still running");
    }

    #[tokio::test]
    async fn capture_stops_an_endless_producer() {
        let d = tempfile::tempdir().unwrap();
        let c = fake(&d, "exec yes");
        let r = tokio::time::timeout(std::time::Duration::from_secs(10), c.capture(&[])).await;
        assert_eq!(r.expect("hung"), Err(ColimaError::OutputTooLarge));
    }

    #[tokio::test]
    async fn failed_stderr_is_cut_to_64_kib_without_blocking_the_child() {
        let d = tempfile::tempdir().unwrap();
        let c = fake(&d, "head -c 300000 /dev/zero | tr '\\0' e 1>&2; exit 2");
        match c.capture(&[]).await {
            Err(ColimaError::Failed {
                code: Some(2),
                stderr,
            }) => assert_eq!(stderr.len(), 64 * 1024),
            other => panic!("{:?}", other.map(|s| s.len())),
        }
        let small = fake(&d, "printf abc 1>&2; exit 2");
        match small.capture(&[]).await {
            Err(ColimaError::Failed { stderr, .. }) => assert_eq!(stderr, "abc"),
            other => panic!("{:?}", other.map(|s| s.len())),
        }
    }

    async fn stream_lines(body: &str) -> Vec<String> {
        let d = tempfile::tempdir().unwrap();
        let c = fake(&d, body);
        let (mut rx, h) = c.run_streaming(vec![]);
        let mut v = vec![];
        while let Some(l) = rx.recv().await {
            v.push(l);
        }
        h.await.unwrap().unwrap();
        v
    }

    #[tokio::test]
    async fn streamed_line_is_cut_to_64_kib_and_next_line_survives() {
        let v = stream_lines("head -c 300000 /dev/zero | tr '\\0' x; echo; echo after").await;
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].len(), 64 * 1024);
        assert!(v[0].bytes().all(|b| b == b'x'));
        assert_eq!(v[1], "after");
    }

    #[tokio::test]
    async fn streamed_line_of_exactly_the_limit_is_kept_whole() {
        let v = stream_lines("head -c 65536 /dev/zero | tr '\\0' x; echo").await;
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].len(), 64 * 1024);
    }

    #[tokio::test]
    async fn streamed_lines_handle_crlf_blank_and_unterminated_last_line() {
        let v = stream_lines("printf 'a\\r\\n\\nb\\nc'").await;
        assert_eq!(v, ["a", "", "b", "c"]);
    }

    #[tokio::test]
    #[ignore = "実際のcolimaが必要"]
    async fn lists_real_instances() {
        let path = std::env::var("PATH").unwrap_or_default();
        let bin = crate::paths::find_binary("colima", &path).expect("colima not found");
        assert!(!Colima::new(bin).list().await.unwrap().is_empty());
    }
}
