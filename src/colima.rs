use crate::i18n::{Lang, Msg};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use tokio::io::{AsyncBufReadExt, BufReader};
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
fn check_profile(p: &str) -> Result<(), ColimaError> {
    let ok = !p.is_empty()
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
pub fn simple_args(action: Action, profile: &str) -> Vec<String> {
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
    v
}

#[derive(Debug, Clone)]
pub struct Colima {
    bin: PathBuf,
}

impl Colima {
    pub fn new(bin: PathBuf) -> Self {
        Self { bin }
    }

    pub async fn list(&self) -> Result<Vec<Instance>, ColimaError> {
        parse_list(&self.capture(&["list", "--json"]).await?)
    }

    pub async fn docker_socket(&self, profile: &str) -> Result<PathBuf, ColimaError> {
        check_profile(profile)?;
        parse_docker_socket(&self.capture(&["status", "--json", "-p", profile]).await?)
    }

    async fn capture(&self, args: &[&str]) -> Result<String, ColimaError> {
        let out = Command::new(&self.bin)
            .args(args)
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| ColimaError::Spawn(e.to_string()))?;
        if !out.status.success() {
            return Err(ColimaError::Failed {
                code: out.status.code(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
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
        (rx, tokio::spawn(stream(bin, args, tx)))
    }
}

async fn stream(
    bin: PathBuf,
    args: Vec<String>,
    tx: mpsc::Sender<String>,
) -> Result<ExitStatus, ColimaError> {
    let mut child = Command::new(bin)
        .args(args)
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

fn forward<R: tokio::io::AsyncRead + Unpin + Send + 'static>(
    r: R,
    tx: mpsc::Sender<String>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(r).lines();
        // 受け手が消えても読み続ける。読むのをやめるとパイプが詰まって子が止まる。
        while let Ok(Some(l)) = lines.next_line().await {
            let _ = tx.send(l).await;
        }
    })
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
        for p in ["", "a b", "../x", ".", "..", "a/b", "日本"] {
            let s = CreateSpec {
                profile: p.into(),
                ..spec()
            };
            assert!(
                matches!(start_args(&s), Err(ColimaError::InvalidProfile(_))),
                "{p:?}"
            );
        }
        for p in ["a", "A-z_0.9", "..a", "a.."] {
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
            simple_args(Action::Delete, "dev"),
            ["delete", "dev", "--force"]
        );
        assert_eq!(simple_args(Action::Start, "dev"), ["start", "dev"]);
        assert_eq!(simple_args(Action::Stop, "dev"), ["stop", "dev"]);
        assert_eq!(simple_args(Action::Restart, "dev"), ["restart", "dev"]);
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
    #[ignore = "実際のcolimaが必要"]
    async fn lists_real_instances() {
        let path = std::env::var("PATH").unwrap_or_default();
        let bin = crate::paths::find_binary("colima", &path).expect("colima not found");
        assert!(!Colima::new(bin).list().await.unwrap().is_empty());
    }
}
