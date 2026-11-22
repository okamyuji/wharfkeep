//! UIスレッドと非同期処理の橋渡し。UIスレッドはCLIもAPIも待たず、要求をchannelへ置いて結果を受けるだけにする。

use super::pages::format_bytes;
use crate::colima::{self, Action, Colima, CreateSpec};
use crate::daemon::monitor::run_action_with;
use crate::docker::actions::{NetDriver, PullProgress, RunSpec, Usage};
use crate::docker::{self, DockerError, Engine};
use crate::i18n::{Lang, Msg};
use crate::paths;
use futures_util::StreamExt;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

/// 外部ターミナルでコマンドを開く関数。試験では実物のターミナルを開かないよう差し替える。
pub type OpenTerminal = Arc<dyn Fn(&[String], &[(String, String)]) -> io::Result<()> + Send + Sync>;

/// 外部ターミナルで開くコマンドの引数と環境変数。
pub type ShellCommand = (Vec<String>, Vec<(String, String)>);

/// コンテナの実行フォームの処理中を表す印。
pub fn run_key(profile: &str) -> String {
    format!("run:{profile}")
}

pub fn image_key(id: &str) -> String {
    format!("image:{id}")
}

pub fn volume_key(name: &str) -> String {
    format!("volume:{name}")
}

pub fn network_key(id: &str) -> String {
    format!("network:{id}")
}

pub fn pull_key(profile: &str) -> String {
    format!("pull:{profile}")
}

pub fn tag_key(profile: &str) -> String {
    format!("tag:{profile}")
}

/// 詳細（inspect）を開けるリソース。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Image,
    Volume,
    Network,
}

pub fn create_volume_key(profile: &str) -> String {
    format!("create-volume:{profile}")
}

pub fn create_network_key(profile: &str) -> String {
    format!("create-network:{profile}")
}

/// 片付けの種類。画面は種類ごとに処理中を持つ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cleanup {
    Images,
    BuildCache,
    Volumes,
    Networks,
}

impl Cleanup {
    pub fn key(self, profile: &str) -> String {
        let what = match self {
            Cleanup::Images => "images",
            Cleanup::BuildCache => "build-cache",
            Cleanup::Volumes => "volumes",
            Cleanup::Networks => "networks",
        };
        format!("prune-{what}:{profile}")
    }
}

/// 片付けで空いた量。ネットワークは容量を持たないので、件数で返す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reclaimed {
    Bytes(u64),
    Networks(usize),
}

pub fn reclaimed_text(r: &Reclaimed, lang: Lang) -> String {
    match r {
        Reclaimed::Bytes(n) => Msg::Freed.text(lang).replace("{size}", &format_bytes(*n)),
        Reclaimed::Networks(n) => Msg::NetworksRemoved
            .text(lang)
            .replace("{n}", &n.to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ContainerOp {
    Start,
    Stop,
    Restart,
    Pause,
    Unpause,
    Remove { force: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Colima {
        action: Action,
        profile: String,
    },
    Create(CreateSpec),
    Container {
        profile: String,
        id: String,
        op: ContainerOp,
    },
    Run {
        profile: String,
        spec: RunSpec,
    },
    /// 返事の印はコンテナのID（行の処理中と同じ）。
    Rename {
        profile: String,
        id: String,
        name: String,
    },
    Inspect {
        profile: String,
        id: String,
    },
    Usage {
        profile: String,
        id: String,
    },
    /// 同じコンテナのログの流れがあれば、置き換える。
    Logs {
        profile: String,
        id: String,
        follow: bool,
    },
    StopLogs {
        id: String,
    },
    /// `container`が`None`ならインスタンスのシェル。
    Shell {
        profile: String,
        container: Option<String>,
    },
    Pull {
        profile: String,
        reference: String,
    },
    /// `size`は消す前の一覧の大きさ。本体まで消えたら、空いた量として知らせる。
    RemoveImage {
        profile: String,
        id: String,
        force: bool,
        size: u64,
    },
    /// `size`は消す前の使用量。取れていなければ`None`。
    RemoveVolume {
        profile: String,
        name: String,
        size: Option<u64>,
    },
    /// ネットワークは名前でなくIDで消す。`my net`のような、名前の検査に通らない名前でも作れるため。
    RemoveNetwork {
        profile: String,
        id: String,
    },
    CreateVolume {
        profile: String,
        name: String,
    },
    CreateNetwork {
        profile: String,
        name: String,
        driver: NetDriver,
    },
    TagImage {
        profile: String,
        id: String,
        repo: String,
        tag: String,
    },
    /// ネットワークの`target`はID。
    InspectResource {
        profile: String,
        kind: ResourceKind,
        target: String,
    },
    PruneImages {
        profile: String,
        dangling: bool,
    },
    PruneBuildCache {
        profile: String,
    },
    PruneVolumes {
        profile: String,
    },
    PruneNetworks {
        profile: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    /// colimaの処理が終わった。失敗なら理由。
    ColimaDone {
        profile: String,
        error: Option<String>,
    },
    /// colimaの出力の1行。
    InstanceLine {
        profile: String,
        line: String,
    },
    /// `key`の処理が終わった。失敗なら理由。キーは`run_key`、コンテナのID。
    OpDone {
        key: String,
        error: Option<String>,
    },
    /// 画面に処理中の印を持たない処理の失敗。
    Failed(String),
    Inspected {
        id: String,
        text: String,
    },
    UsageSample {
        id: String,
        usage: Usage,
    },
    LogLine {
        id: String,
        line: String,
    },
    /// 失敗ではない知らせ。片付けで空いた量など。
    Notice(String),
    PullProgress {
        profile: String,
        progress: PullProgress,
    },
    ResourceInspected {
        kind: ResourceKind,
        target: String,
        text: String,
    },
}

pub struct Bridge {
    requests: UnboundedSender<Request>,
    replies: Receiver<Reply>,
}

/// 非同期処理の側が持つ端。
pub struct WorkerEnds {
    pub requests: UnboundedReceiver<Request>,
    pub replies: Sender<Reply>,
}

impl Bridge {
    pub fn pair() -> (Bridge, WorkerEnds) {
        let (req_tx, req_rx) = unbounded_channel();
        let (rep_tx, rep_rx) = channel();
        (
            Bridge {
                requests: req_tx,
                replies: rep_rx,
            },
            WorkerEnds {
                requests: req_rx,
                replies: rep_tx,
            },
        )
    }

    /// 別スレッドのtokioで要求を処理する。結果が届くたびに`repaint`を呼ぶ。
    pub fn spawn(
        colima: Colima,
        lang: Lang,
        repaint: impl Fn() + Send + Sync + 'static,
        open_terminal: OpenTerminal,
    ) -> io::Result<Bridge> {
        let (bridge, ends) = Bridge::pair();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let cx = Arc::new(Cx {
            colima,
            lang,
            replies: ends.replies.clone(),
            repaint: Arc::new(repaint),
            open_terminal,
        });
        std::thread::Builder::new()
            .name("gui-bridge".into())
            .spawn(move || rt.block_on(serve(ends.requests, cx)))?;
        Ok(bridge)
    }

    pub fn send(&self, request: Request) {
        // 処理側が止まっていれば送れないが、その場合は返事も来ないので、画面は処理中のまま残る。
        let _ = self.requests.send(request);
    }

    pub fn try_recv(&self) -> Option<Reply> {
        self.replies.try_recv().ok()
    }
}

/// 処理側が共有するもの。
struct Cx {
    colima: Colima,
    lang: Lang,
    replies: Sender<Reply>,
    repaint: Arc<dyn Fn() + Send + Sync>,
    open_terminal: OpenTerminal,
}

impl Cx {
    fn send(&self, reply: Reply) {
        if self.replies.send(reply).is_ok() {
            (self.repaint)();
        }
    }

    async fn engine(&self, profile: &str) -> Result<Engine, String> {
        let socket = self
            .colima
            .docker_socket(profile)
            .await
            .map_err(|e| e.message(self.lang))?;
        Engine::connect(&socket).map_err(|e| e.message(self.lang))
    }
}

async fn serve(mut requests: UnboundedReceiver<Request>, cx: Arc<Cx>) {
    // コンテナごとのログの流れ。止めるために手元に持つ。
    let mut logs: HashMap<String, JoinHandle<()>> = HashMap::new();
    while let Some(request) = requests.recv().await {
        match request {
            Request::Logs {
                profile,
                id,
                follow,
            } => {
                if let Some(old) = logs.remove(&id) {
                    old.abort();
                }
                logs.insert(
                    id.clone(),
                    tokio::spawn(stream_logs(cx.clone(), profile, id, follow)),
                );
            }
            Request::StopLogs { id } => {
                if let Some(old) = logs.remove(&id) {
                    old.abort();
                }
            }
            other => {
                tokio::spawn(handle(cx.clone(), other));
            }
        }
    }
}

async fn stream_logs(cx: Arc<Cx>, profile: String, id: String, follow: bool) {
    let engine = match cx.engine(&profile).await {
        Ok(e) => e,
        Err(m) => return cx.send(Reply::Failed(m)),
    };
    let mut lines = std::pin::pin!(engine.logs(&id, follow));
    while let Some(line) = lines.next().await {
        match line {
            Ok(line) => cx.send(Reply::LogLine {
                id: id.clone(),
                line,
            }),
            Err(e) => return cx.send(Reply::Failed(e.message(cx.lang))),
        }
    }
}

/// 接続して`f`を走らせる。ソケットの取得、接続、操作のどれが失敗しても、文言にして返す。
async fn with_engine<T, Fut>(
    cx: &Cx,
    profile: &str,
    f: impl FnOnce(Engine) -> Fut,
) -> Result<T, String>
where
    Fut: std::future::Future<Output = Result<T, DockerError>>,
{
    let engine = cx.engine(profile).await?;
    f(engine).await.map_err(|e| e.message(cx.lang))
}

/// どの経路でも返事を1つ返す。返さないと、画面が処理中のまま残る。
async fn handle(cx: Arc<Cx>, request: Request) {
    let cx = &*cx;
    match request {
        Request::Colima { action, profile } => {
            let args = colima::simple_args(action, &profile);
            colima_task(cx, profile, args).await;
        }
        Request::Create(spec) => {
            let args = colima::start_args(&spec);
            colima_task(cx, spec.profile, args).await;
        }
        Request::Container { profile, id, op } => {
            let target = id.clone();
            let result = with_engine(
                cx,
                &profile,
                |e| async move { apply(&e, &target, op).await },
            )
            .await;
            // コンテナは一覧に大きさを持たないので、容量なしで知らせる。
            if result.is_ok() && matches!(op, ContainerOp::Remove { .. }) {
                cx.send(Reply::Notice(
                    Msg::ContainerDeleted.text(cx.lang).to_string(),
                ));
            }
            cx.send(done(id, result));
        }
        Request::Rename { profile, id, name } => {
            let target = id.clone();
            let result = with_engine(
                cx,
                &profile,
                |e| async move { e.rename(&target, &name).await },
            )
            .await;
            cx.send(done(id, result));
        }
        Request::Run { profile, spec } => {
            let result = with_engine(cx, &profile, |e| async move { e.run(spec).await }).await;
            cx.send(done(run_key(&profile), result));
        }
        Request::Inspect { profile, id } => {
            let target = id.clone();
            let result =
                with_engine(cx, &profile, |e| async move { e.inspect(&target).await }).await;
            cx.send(result.map_or_else(Reply::Failed, |json| Reply::Inspected {
                id,
                text: serde_json::to_string_pretty(&json).unwrap_or_default(),
            }));
        }
        Request::Usage { profile, id } => {
            let target = id.clone();
            let result =
                with_engine(cx, &profile, |e| async move { e.stats_once(&target).await }).await;
            cx.send(result.map_or_else(Reply::Failed, |usage| Reply::UsageSample { id, usage }));
        }
        Request::Shell { profile, container } => {
            if let Err(m) = open_shell(cx, &profile, container).await {
                cx.send(Reply::Failed(m));
            }
        }
        Request::Pull { profile, reference } => pull(cx, profile, reference).await,
        other => handle_resource(cx, other).await,
    }
}

/// イメージ、ボリューム、ネットワークの要求。どれも返事を1つ返す。
async fn handle_resource(cx: &Cx, request: Request) {
    match request {
        Request::RemoveImage {
            profile,
            id,
            force,
            size,
        } => remove_image(cx, profile, id, force, size).await,
        Request::RemoveVolume {
            profile,
            name,
            size,
        } => remove_volume(cx, profile, name, size).await,
        Request::RemoveNetwork { profile, id } => {
            let target = id.clone();
            let result = with_engine(
                cx,
                &profile,
                |e| async move { e.remove_network(&target).await },
            )
            .await;
            cx.send(done(network_key(&id), result));
        }
        Request::CreateVolume { profile, name } => {
            let result = with_engine(
                cx,
                &profile,
                |e| async move { e.create_volume(&name).await },
            )
            .await;
            cx.send(done(create_volume_key(&profile), result));
        }
        Request::CreateNetwork {
            profile,
            name,
            driver,
        } => {
            let result = with_engine(cx, &profile, |e| async move {
                e.create_network(&name, driver).await
            })
            .await;
            cx.send(done(create_network_key(&profile), result));
        }
        Request::TagImage {
            profile,
            id,
            repo,
            tag,
        } => {
            let result = with_engine(
                cx,
                &profile,
                |e| async move { e.tag(&id, &repo, &tag).await },
            )
            .await;
            cx.send(done(tag_key(&profile), result));
        }
        Request::InspectResource {
            profile,
            kind,
            target,
        } => {
            let id = target.clone();
            let result = with_engine(cx, &profile, |e| async move {
                match kind {
                    ResourceKind::Image => e.inspect_image(&id).await,
                    ResourceKind::Volume => e.inspect_volume(&id).await,
                    ResourceKind::Network => e.inspect_network(&id).await,
                }
            })
            .await;
            cx.send(
                result.map_or_else(Reply::Failed, |json| Reply::ResourceInspected {
                    kind,
                    target,
                    text: serde_json::to_string_pretty(&json).unwrap_or_default(),
                }),
            );
        }
        Request::PruneImages { profile, dangling } => {
            let result = with_engine(cx, &profile, |e| async move {
                e.prune_images(dangling).await.map(Reclaimed::Bytes)
            })
            .await;
            finish_prune(cx, Cleanup::Images.key(&profile), result);
        }
        Request::PruneBuildCache { profile } => {
            let result = with_engine(cx, &profile, |e| async move {
                e.prune_build_cache().await.map(Reclaimed::Bytes)
            })
            .await;
            finish_prune(cx, Cleanup::BuildCache.key(&profile), result);
        }
        Request::PruneVolumes { profile } => {
            let result = with_engine(cx, &profile, |e| async move {
                e.prune_volumes().await.map(Reclaimed::Bytes)
            })
            .await;
            finish_prune(cx, Cleanup::Volumes.key(&profile), result);
        }
        Request::PruneNetworks { profile } => {
            let result = with_engine(cx, &profile, |e| async move {
                e.prune_networks().await.map(Reclaimed::Networks)
            })
            .await;
            finish_prune(cx, Cleanup::Networks.key(&profile), result);
        }
        // `serve`と`handle`が自分で扱う。
        _ => {}
    }
}

/// 本体まで消えたら覚えておいた大きさを、タグだけ外れたらそのことを知らせてから、終わりを返す。
async fn remove_image(cx: &Cx, profile: String, id: String, force: bool, size: u64) {
    let target = id.clone();
    let result = with_engine(cx, &profile, |e| async move {
        e.remove_image(&target, force).await
    })
    .await;
    if let Ok(deleted) = result {
        cx.send(Reply::Notice(if deleted {
            reclaimed_text(&Reclaimed::Bytes(size), cx.lang)
        } else {
            Msg::Untagged.text(cx.lang).replace("{name}", &id)
        }));
    }
    cx.send(done(image_key(&id), result));
}

/// 使用量が分かれば空いた量を、分からなければ名前を知らせてから、終わりを返す。
async fn remove_volume(cx: &Cx, profile: String, name: String, size: Option<u64>) {
    let target = name.clone();
    let result = with_engine(
        cx,
        &profile,
        |e| async move { e.remove_volume(&target).await },
    )
    .await;
    if result.is_ok() {
        cx.send(Reply::Notice(match size {
            Some(n) => reclaimed_text(&Reclaimed::Bytes(n), cx.lang),
            None => Msg::DeletedItem.text(cx.lang).replace("{name}", &name),
        }));
    }
    cx.send(done(volume_key(&name), result));
}

/// 成功なら空いた量を知らせてから、終わりを返す。
fn finish_prune(cx: &Cx, key: String, result: Result<Reclaimed, String>) {
    if let Ok(r) = &result {
        cx.send(Reply::Notice(reclaimed_text(r, cx.lang)));
    }
    cx.send(done(key, result));
}

/// 層ごとの進捗を流し、終わると返事を1つ返す。
async fn pull(cx: &Cx, profile: String, reference: String) {
    let result = pull_stream(cx, &profile, &reference).await;
    cx.send(done(pull_key(&profile), result));
}

async fn pull_stream(cx: &Cx, profile: &str, reference: &str) -> Result<(), String> {
    let engine = cx.engine(profile).await?;
    let mut events = std::pin::pin!(engine.pull(reference));
    while let Some(event) = events.next().await {
        let progress = event.map_err(|e| e.message(cx.lang))?;
        cx.send(Reply::PullProgress {
            profile: profile.to_string(),
            progress,
        });
    }
    Ok(())
}

fn done<T>(key: String, result: Result<T, String>) -> Reply {
    Reply::OpDone {
        key,
        error: result.err(),
    }
}

async fn apply(engine: &Engine, id: &str, op: ContainerOp) -> Result<(), DockerError> {
    match op {
        ContainerOp::Start => engine.start(id).await,
        ContainerOp::Stop => engine.stop(id).await,
        ContainerOp::Restart => engine.restart(id).await,
        ContainerOp::Pause => engine.pause(id).await,
        ContainerOp::Unpause => engine.unpause(id).await,
        ContainerOp::Remove { force } => engine.remove(id, force).await,
    }
}

/// colimaを走らせ、出力の各行を`InstanceLine`で流す。終わると`ColimaDone`を1つ返す。
async fn colima_task(cx: &Cx, profile: String, args: Result<Vec<String>, colima::ColimaError>) {
    let error = match args {
        Ok(args) => {
            let action = args.first().cloned().unwrap_or_default();
            let on_line = |line: &str| {
                cx.send(Reply::InstanceLine {
                    profile: profile.clone(),
                    line: line.to_string(),
                })
            };
            let error = run_action_with(
                cx.colima.clone(),
                args,
                Arc::new(Notify::new()),
                cx.lang,
                on_line,
            )
            .await;
            if error.is_none() {
                cx.send(Reply::Notice(
                    Msg::ColimaSucceeded
                        .text(cx.lang)
                        .replace("{profile}", &profile)
                        .replace("{action}", &action),
                ));
            }
            error
        }
        Err(e) => Some(e.message(cx.lang)),
    };
    cx.send(Reply::ColimaDone { profile, error });
}

/// コンテナのシェルは`docker exec -it <id> sh`。選んだインスタンスのソケットを`DOCKER_HOST`に入れる。
pub fn container_shell(
    docker_bin: &Path,
    socket: &Path,
    id: &str,
) -> Result<ShellCommand, DockerError> {
    docker::check_name(id)?;
    let argv = vec![
        docker_bin.display().to_string(),
        "exec".into(),
        "-it".into(),
        id.into(),
        "sh".into(),
    ];
    let env = vec![("DOCKER_HOST".into(), format!("unix://{}", socket.display()))];
    Ok((argv, env))
}

async fn open_shell(cx: &Cx, profile: &str, container: Option<String>) -> Result<(), String> {
    let lang = cx.lang;
    let (argv, env) = match container {
        None => {
            let args = colima::ssh_args(profile).map_err(|e| e.message(lang))?;
            let colima_bin = cx.colima.bin().display().to_string();
            (
                std::iter::once(colima_bin).chain(args).collect(),
                Vec::new(),
            )
        }
        Some(id) => {
            let socket = cx
                .colima
                .docker_socket(profile)
                .await
                .map_err(|e| e.message(lang))?;
            let path_env = std::env::var("PATH").unwrap_or_default();
            let docker_bin = paths::find_binary("docker", &path_env)
                .unwrap_or_else(|| std::path::PathBuf::from("docker"));
            container_shell(&docker_bin, &socket, &id).map_err(|e| e.message(lang))?
        }
    };
    let open = cx.open_terminal.clone();
    // 外部ターミナルを開く処理は待つので、非同期の実行を止めないよう別スレッドで走らせる。
    tokio::task::spawn_blocking(move || open(&argv, &env))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("{}: {e}", Msg::TerminalFailed.text(lang)))
}

#[cfg(test)]
mod tests;
