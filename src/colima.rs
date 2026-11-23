use crate::i18n::{Lang, Msg};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
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

impl VmType {
    pub const ALL: [VmType; 2] = [VmType::Vz, VmType::Qemu];

    /// `colima start --vm-type`の値。
    pub fn as_str(self) -> &'static str {
        match self {
            VmType::Vz => "vz",
            VmType::Qemu => "qemu",
        }
    }
}

impl Arch {
    pub const ALL: [Arch; 2] = [Arch::Aarch64, Arch::X86_64];

    /// このMacのアーキテクチャ。
    pub fn host() -> Arch {
        if cfg!(target_arch = "aarch64") {
            Arch::Aarch64
        } else {
            Arch::X86_64
        }
    }

    /// `colima start --arch`の値。
    pub fn as_str(self) -> &'static str {
        match self {
            Arch::Aarch64 => "aarch64",
            Arch::X86_64 => "x86_64",
        }
    }
}

impl MountType {
    pub const ALL: [MountType; 3] = [MountType::Virtiofs, MountType::Sshfs, MountType::NineP];

    /// `colima start --mount-type`の値。
    pub fn as_str(self) -> &'static str {
        match self {
            MountType::Virtiofs => "virtiofs",
            MountType::Sshfs => "sshfs",
            MountType::NineP => "9p",
        }
    }
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
/// 名前は colima の全操作がこの検査を通る。
pub fn check_profile(p: &str) -> Result<(), ColimaError> {
    let ok = !p.is_empty()
        && !p.starts_with('-')
        && p.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && !p.chars().all(|c| c == '.')
        // colima は `colima` を既定のインスタンスと読み、`colima-` を1回だけ外す（大文字と小文字を区別する）。
        // 一覧の名前と操作で解決される名前がずれ、`colima delete colima-dev` が別の VM を消す。
        && p != "colima"
        && !p.starts_with("colima-");
    ok.then_some(())
        .ok_or_else(|| ColimaError::InvalidProfile(p.into()))
}

/// colima が名前を解決したあとのインスタンス名（config/profile.go）。`colima` と空は `default`、
/// 先頭の `colima-` は外す。`colima-old` も `old` を指す。
pub fn short_name(p: &str) -> &str {
    match p {
        "" | "colima" => "default",
        _ => p.strip_prefix("colima-").unwrap_or(p),
    }
}

/// 作ろうとする名前が既存のインスタンスと同じものを指すか。`colima list` は解決後の名前を出すので、
/// 既存の側はそのまま比べる。macOS の既定のファイルシステムは大文字と小文字を区別しないので、
/// colima の置き場 `~/.colima/<名前>` が重なる名前も同じと見る。
pub fn same_instance(new: &str, existing: &str) -> bool {
    short_name(new).eq_ignore_ascii_case(existing)
}

pub fn start_args(spec: &CreateSpec) -> Result<Vec<String>, ColimaError> {
    check_profile(&spec.profile)?;
    // 大文字と小文字を区別しない macOS の置き場では `Default` や `COLIMA` も既定のインスタンスと重なる。
    if spec.profile != "default"
        && ["colima", "default"]
            .iter()
            .any(|d| spec.profile.eq_ignore_ascii_case(d))
    {
        return Err(ColimaError::InvalidProfile(spec.profile.clone()));
    }
    if spec.cpus == 0 {
        return Err(ColimaError::InvalidCpus);
    }
    if !spec.memory_gib.is_finite() || spec.memory_gib <= 0.0 {
        return Err(ColimaError::InvalidMemory(spec.memory_gib));
    }
    if spec.disk_gib == 0 {
        return Err(ColimaError::InvalidDisk);
    }
    let (vm, arch, mount) = (
        spec.vm_type.as_str(),
        spec.arch.as_str(),
        spec.mount_type.as_str(),
    );
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

/// インスタンスのシェルを開く引数。コマンドを付けないので`--`は要らない。
pub fn ssh_args(profile: &str) -> Result<Vec<String>, ColimaError> {
    check_profile(profile)?;
    Ok(["ssh", "-p", profile].map(String::from).to_vec())
}

/// `list` と `status` の待ち時間。`start` などの長い処理は `run_streaming` を通るので対象外。
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);
/// JSON を読むための stdout の上限。`colima list --json` は数 KiB なので十分に大きい。
const STDOUT_LIMIT: usize = 1024 * 1024;
/// 1行と stderr の保持量の上限。超えた分は読み捨てる。
const LINE_LIMIT: usize = 64 * 1024;

/// `run_streaming` が出力を書かせるファイルの名前の頭。デーモンの起動時の掃除もこれで見分ける。
pub const OUTPUT_PREFIX: &str = "colima-output.";
/// `run_streaming` が出力のファイルを読み直す間隔。
const OUTPUT_POLL: Duration = Duration::from_millis(100);
static OUTPUT_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct Colima {
    bin: PathBuf,
    /// 子に渡す`PATH`。
    path_env: OsString,
    /// `run_streaming` の出力のファイルを置くディレクトリ。
    out_dir: PathBuf,
}

impl Colima {
    /// 出力のファイルはデータ置き場に置く。ホームが分からないときだけ一時ディレクトリに置く。
    pub fn new(bin: PathBuf) -> Self {
        let out_dir = crate::paths::data_dir().unwrap_or_else(std::env::temp_dir);
        Self::with_out_dir(bin, out_dir)
    }

    pub fn with_out_dir(bin: PathBuf, out_dir: PathBuf) -> Self {
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
        Self {
            bin,
            path_env,
            out_dir,
        }
    }

    pub fn bin(&self) -> &Path {
        &self.bin
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

    /// stdout と stderr の行を書かれた順に1本の受け口へ流す。終了状態は子が終わって出力を読み切ったあとに返る。
    /// 呼び手（GUI）が先に終わっても、子は最後まで進む。
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
        let out_dir = self.out_dir.clone();
        (rx, tokio::spawn(stream(bin, path_env, out_dir, args, tx)))
    }
}

/// 子の出力を書かせるファイルを開き、書き手と読み手を返す。名前はすぐ消すので、親が先に終わっても残らない。
/// 読み手は別に開き直し、子の書き込み位置と読む位置を分ける。
fn open_output(dir: &Path) -> std::io::Result<(std::fs::File, std::fs::File)> {
    use std::os::unix::fs::OpenOptionsExt;
    crate::paths::ensure_private_dir(dir)?;
    let n = OUTPUT_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("{OUTPUT_PREFIX}{}.{n}.tmp", std::process::id()));
    let writer = std::fs::OpenOptions::new()
        .append(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    let reader = std::fs::File::open(&path);
    std::fs::remove_file(&path)?;
    Ok((writer, reader?))
}

// パイプにしないのは、親が終わると kill_on_drop か SIGPIPE で子も止まるため。
async fn stream(
    bin: PathBuf,
    path_env: OsString,
    out_dir: PathBuf,
    args: Vec<String>,
    tx: mpsc::Sender<String>,
) -> Result<ExitStatus, ColimaError> {
    let spawn_err = |e: std::io::Error| ColimaError::Spawn(e.to_string());
    let (writer, mut reader) = open_output(&out_dir).map_err(spawn_err)?;
    let mut child = Command::new(bin)
        .args(args)
        .env("PATH", path_env)
        .stdin(Stdio::null())
        .stdout(writer.try_clone().map_err(spawn_err)?)
        .stderr(writer)
        .spawn()
        .map_err(spawn_err)?;
    let mut lines = LineSplitter::default();
    loop {
        let status = tokio::select! {
            s = child.wait() => Some(s.map_err(spawn_err)?),
            () = tokio::time::sleep(OUTPUT_POLL) => None,
        };
        pump(&mut reader, &mut lines, &tx)
            .await
            .map_err(spawn_err)?;
        if let Some(status) = status {
            if let Some(l) = lines.finish() {
                let _ = tx.send(l).await;
            }
            return Ok(status);
        }
    }
}

/// 1回の読み込みで確保する大きさ。改行なしで大量に出す子でも、メモリは `READ_CHUNK` を超えて増えない。
const READ_CHUNK: usize = 64 * 1024;

/// 今ある分を `READ_CHUNK` ずつ読み切り、行にして流す。
async fn pump(
    reader: &mut impl std::io::Read,
    lines: &mut LineSplitter,
    tx: &mpsc::Sender<String>,
) -> std::io::Result<()> {
    let mut buf = vec![0u8; READ_CHUNK];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        for l in lines.push(&buf[..n]) {
            let _ = tx.send(l).await;
        }
    }
}

/// 伸びていくファイルから読んだバイト列を行に分ける。`\n` と `\r\n` で区切る。
/// 1行の保持量は `LINE_LIMIT` バイトまでで、超えた分は捨てる。
#[derive(Default)]
struct LineSplitter {
    line: Vec<u8>,
}

impl LineSplitter {
    /// 改行まで届いた行を返す。改行がまだ来ていない行は次の `push` まで持つ。
    fn push(&mut self, mut bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(nl) = bytes.iter().position(|&b| b == b'\n') {
            self.keep(&bytes[..nl]);
            let mut line = std::mem::take(&mut self.line);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            out.push(String::from_utf8_lossy(&line).into_owned());
            bytes = &bytes[nl + 1..];
        }
        self.keep(bytes);
        out
    }

    fn keep(&mut self, bytes: &[u8]) {
        let room = LINE_LIMIT.saturating_sub(self.line.len());
        self.line.extend_from_slice(&bytes[..bytes.len().min(room)]);
    }

    /// 改行で終わらなかった最後の行。
    fn finish(self) -> Option<String> {
        (!self.line.is_empty()).then(|| String::from_utf8_lossy(&self.line).into_owned())
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn short_name_follows_colima_profile_resolution() {
        assert_eq!(short_name(""), "default");
        assert_eq!(short_name("colima"), "default");
        assert_eq!(short_name("colima-old"), "old");
        assert_eq!(short_name("colima-default"), "default");
        assert_eq!(short_name("old"), "old");
        assert_eq!(short_name("my-colima"), "my-colima");
    }

    #[test]
    fn same_instance_sees_aliases_and_case_as_the_same() {
        for (new, existing) in [
            ("old", "old"),
            ("colima-old", "old"),
            ("colima", "default"),
            ("colima-default", "default"),
            ("OLD", "old"),
            ("colima-colima", "colima"),
        ] {
            assert!(same_instance(new, existing), "{new} {existing}");
        }
        // colima は接頭辞を大文字と小文字を区別して外すので、`Colima-Old` は `~/.colima/Colima-Old` になる。
        for (new, existing) in [
            ("old2", "old"),
            ("dev", "default"),
            ("colima-x", "old"),
            ("Colima-Old", "old"),
        ] {
            assert!(!same_instance(new, existing), "{new} {existing}");
        }
    }

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
            "",
            "a b",
            "../x",
            ".",
            "..",
            "a/b",
            "日本",
            "-h",
            "--force",
            "-",
            // colima が外す接頭辞のあとが空や `.` だけだと、置き場がホームなどを指す。
            "colima-",
            "colima-.",
            "colima-..",
            // 一覧に `colima` や `default` と出て、行の操作が既定のインスタンスに向く。
            "colima-colima",
            "colima-default",
            "colima-Default",
            "Default",
            "COLIMA",
            "colima-COLIMA",
            // colima は `colima-` を1回だけ外すので、一覧に `colima-dev` と出る。行の削除が `dev` を消す。
            "colima-colima-dev",
            "colima-colima-default",
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
        for p in ["a", "A-z_0.9", "..a", "a..", "a-b", "default"] {
            let s = CreateSpec {
                profile: p.into(),
                ..spec()
            };
            assert!(start_args(&s).is_ok(), "{p:?}");
        }
    }

    /// colima は `colima-` を1回だけ外し、`colima` を既定のインスタンスと読む。
    /// 一覧の行の名前と、colima が操作で解決する名前がずれる。
    const SHADOWING: [&str; 6] = [
        "colima",
        "colima-dev",
        "colima-colima",
        "colima-default",
        "colima-colima-dev",
        "colima-Colima-dev",
    ];
    const PLAIN: [&str; 4] = ["default", "dev", "Colima-dev", "Colima"];

    #[test]
    fn every_command_builder_rejects_names_colima_resolves_elsewhere() {
        for p in SHADOWING {
            let spec = CreateSpec {
                profile: p.into(),
                ..spec()
            };
            assert!(
                matches!(start_args(&spec), Err(ColimaError::InvalidProfile(_))),
                "start {p}"
            );
            for a in [Action::Start, Action::Stop, Action::Restart, Action::Delete] {
                assert!(
                    matches!(simple_args(a, p), Err(ColimaError::InvalidProfile(_))),
                    "{a:?} {p}"
                );
            }
            assert!(
                matches!(ssh_args(p), Err(ColimaError::InvalidProfile(_))),
                "ssh {p}"
            );
        }
    }

    #[test]
    fn every_command_builder_accepts_plain_names() {
        for p in PLAIN {
            // `Colima` と `Colima-dev` は大文字始まりで、colima は別名にしない。
            let spec = CreateSpec {
                profile: p.into(),
                ..spec()
            };
            let ok = p != "Colima";
            assert_eq!(start_args(&spec).is_ok(), ok, "start {p}");
            assert!(simple_args(Action::Stop, p).is_ok(), "simple {p}");
            assert_eq!(ssh_args(p).unwrap(), ["ssh", "-p", p]);
        }
    }

    #[tokio::test]
    async fn docker_socket_rejects_shadowing_names_before_running_colima() {
        let d = tempfile::tempdir().unwrap();
        let marker = d.path().join("ran");
        let c = fake(
            &d,
            &format!(
                "touch '{}'; echo '{{\"docker_socket\":\"unix:///s/d.sock\"}}'",
                marker.display()
            ),
        );
        for p in SHADOWING {
            assert!(
                matches!(
                    c.docker_socket(p).await,
                    Err(ColimaError::InvalidProfile(_))
                ),
                "{p}"
            );
        }
        assert!(!marker.exists());
        for p in PLAIN {
            assert_eq!(
                c.docker_socket(p).await.unwrap(),
                PathBuf::from("/s/d.sock"),
                "{p}"
            );
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
        for p in ["-h", "--force", "", "a b", "..", "colima-", "colima-.."] {
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
        let d = tempfile::tempdir().unwrap();
        let c = Colima::with_out_dir(PathBuf::from("/bin/sh"), d.path().to_path_buf());
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
        let d = tempfile::tempdir().unwrap();
        let c = Colima::with_out_dir(PathBuf::from("/nonexistent/colima"), d.path().to_path_buf());
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
        Colima::with_out_dir(p, dir.path().to_path_buf())
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
        // mv で pid を一度に見せる。リダイレクトだけだと、空の pid ファイルを読む瞬間がある。
        let c = fake(
            &d,
            &format!(
                "echo $$ > '{f}.tmp' && mv '{f}.tmp' '{f}'; exec sleep 30",
                f = pid_file.display()
            ),
        );
        let started = std::time::Instant::now();
        // 制限は、負荷でシェルの起動が遅れても pid を書き終える長さにする。
        let r = c
            .capture_within(&[], std::time::Duration::from_secs(2))
            .await;
        assert_eq!(r, Err(ColimaError::Timeout));
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        let mut pid = None;
        for _ in 0..40 {
            if let Ok(p) = std::fs::read_to_string(&pid_file)
                && let Ok(n) = p.trim().parse::<u32>()
            {
                pid = Some(n.to_string());
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let pid = pid.expect("the fake colima never wrote its pid");
        let alive = || {
            std::process::Command::new("kill")
                .args(["-0", &pid])
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

    #[test]
    fn new_puts_the_output_in_the_data_dir() {
        let c = Colima::new(PathBuf::from("/bin/sh"));
        assert_eq!(Some(c.out_dir), crate::paths::data_dir());
    }

    #[test]
    fn splitter_keeps_a_line_until_its_newline_arrives() {
        let mut s = LineSplitter::default();
        assert!(s.push(b"ab").is_empty());
        assert_eq!(s.push(b"c\r"), Vec::<String>::new());
        assert_eq!(s.push(b"\nd\n\ne"), ["abc", "d", ""]);
        assert_eq!(s.finish().as_deref(), Some("e"));
        assert_eq!(LineSplitter::default().finish(), None);
    }

    #[test]
    fn splitter_cuts_a_line_split_over_pushes_at_the_limit() {
        let mut s = LineSplitter::default();
        assert!(s.push(&vec![b'x'; LINE_LIMIT - 1]).is_empty());
        assert!(s.push(b"yz").is_empty());
        let v = s.push(b"\nnext");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].len(), LINE_LIMIT);
        assert!(v[0].ends_with("xy"));
        assert_eq!(s.finish().as_deref(), Some("next"));
    }

    #[tokio::test]
    async fn streamed_output_arrives_whole_and_leaves_no_file() {
        let d = tempfile::tempdir().unwrap();
        let c = fake(
            &d,
            "echo one; sleep 0.3; printf 'tw'; sleep 0.3; echo o 1>&2; echo three; exit 4",
        );
        let (mut rx, h) = c.run_streaming(vec![]);
        let mut v = vec![];
        while let Some(l) = rx.recv().await {
            v.push(l);
        }
        assert_eq!(v, ["one", "two", "three"]);
        assert_eq!(h.await.unwrap().unwrap().code(), Some(4));
        let names: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["colima"]);
    }

    #[test]
    fn output_dir_is_made_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let out = d.path().join("data");
        let (w, _r) = open_output(&out).unwrap();
        let mode = |m: std::fs::Metadata| m.permissions().mode() & 0o777;
        assert_eq!(mode(std::fs::metadata(&out).unwrap()), 0o700);
        assert_eq!(mode(w.metadata().unwrap()), 0o600);
        assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn run_streaming_reports_an_unusable_output_dir() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("file");
        std::fs::write(&file, "").unwrap();
        let c = Colima::with_out_dir(PathBuf::from("/bin/sh"), file.join("sub"));
        let (mut rx, h) = c.run_streaming(vec![]);
        assert!(rx.recv().await.is_none());
        assert!(matches!(h.await.unwrap(), Err(ColimaError::Spawn(_))));
    }

    /// GUI の終了を、ランタイムごと捨てて真似る。受け口と task だけを捨てても、読み手の task は残る。
    #[test]
    fn streamed_colima_runs_to_the_end_after_the_runtime_is_dropped() {
        let d = tempfile::tempdir().unwrap();
        let marker = d.path().join("done");
        let c = fake(
            &d,
            &format!(
                "echo begin; sleep 1; echo more; echo more 1>&2; touch '{}'",
                marker.display()
            ),
        );
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (mut rx, h) = rt.block_on(async { c.run_streaming(vec![]) });
        assert_eq!(rt.block_on(rx.recv()).as_deref(), Some("begin"));
        drop((rx, h, rt));
        for _ in 0..100 {
            if marker.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("colima stopped when the runtime was dropped");
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

    /// 読み込みの要求の長さを記録する Reader。`data` を先頭から返す。
    struct Recording<'a> {
        data: &'a [u8],
        asked: Vec<usize>,
    }

    impl std::io::Read for Recording<'_> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.asked.push(buf.len());
            let n = buf.len().min(self.data.len());
            buf[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    #[tokio::test]
    async fn pump_reads_in_chunks_and_keeps_lines_intact() {
        let mut data = vec![b'x'; 1024 * 1024];
        data.extend_from_slice(b"\r\nnext\nlast");
        let mut r = Recording {
            data: &data,
            asked: vec![],
        };
        let (tx, mut rx) = mpsc::channel(8);
        let mut lines = LineSplitter::default();
        pump(&mut r, &mut lines, &tx).await.unwrap();
        drop(tx);
        assert!(r.asked.iter().all(|&n| n == READ_CHUNK), "{:?}", r.asked);
        assert_eq!(r.asked.len(), 1024 / 64 + 2);
        let first = rx.recv().await.unwrap();
        assert_eq!(first.len(), LINE_LIMIT);
        assert_eq!(rx.recv().await.as_deref(), Some("next"));
        assert!(rx.recv().await.is_none());
        assert_eq!(lines.finish().as_deref(), Some("last"));
    }

    #[tokio::test]
    async fn pump_returns_the_read_error() {
        struct Broken;
        impl std::io::Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("boom"))
            }
        }
        let (tx, _rx) = mpsc::channel(1);
        let e = pump(&mut Broken, &mut LineSplitter::default(), &tx).await;
        assert_eq!(e.unwrap_err().to_string(), "boom");
    }

    #[tokio::test]
    async fn a_megabyte_without_a_newline_arrives_as_one_cut_line() {
        let v = stream_lines("head -c 1048576 /dev/zero | tr '\\0' x").await;
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].len(), LINE_LIMIT);
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
