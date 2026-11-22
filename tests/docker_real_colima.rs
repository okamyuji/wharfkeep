//! 実機の colima（`default`）に対する試験。`cargo test -- --ignored` で動かす。

use futures_util::StreamExt;
use std::path::PathBuf;
use std::process::Command;
use wharfkeep::colima::parse_docker_socket;
use wharfkeep::docker::Engine;
use wharfkeep::docker::actions::{NetDriver, RestartPolicy, RunSpec, parse_port_map};

fn socket() -> PathBuf {
    let out = Command::new("colima")
        .args(["status", "--json"])
        .output()
        .expect("colima を実行できない");
    parse_docker_socket(&String::from_utf8_lossy(&out.stdout)).expect("docker_socket を読めない")
}

/// 表明が失敗しても、作ったものを残さない。`kind` は docker のサブコマンド（container、volume、network）。
struct Cleanup {
    socket: PathBuf,
    kind: &'static str,
    name: String,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .arg("-H")
            .arg(format!("unix://{}", self.socket.display()))
            .args([self.kind, "rm", "-f", &self.name])
            .output();
    }
}

fn spec(image: &str, name: &str, cmd: Option<Vec<String>>, ports: Vec<&str>) -> RunSpec {
    RunSpec {
        image: image.into(),
        name: Some(name.into()),
        ports: ports
            .into_iter()
            .map(|p| parse_port_map(p).unwrap())
            .collect(),
        env: vec![("WK_TEST".into(), "1".into())],
        mounts: vec![],
        restart: RestartPolicy::No,
        cmd,
    }
}

async fn pull_image(engine: &Engine, image: &str) {
    let mut pull = Box::pin(engine.pull(image));
    while let Some(p) = pull.next().await {
        p.unwrap();
    }
}

async fn has(engine: &Engine, name: &str) -> bool {
    let s = engine.summary().await.unwrap();
    s.containers.iter().any(|c| c.name == name)
}

#[tokio::test]
#[ignore = "実機の colima が要る"]
async fn container_lifecycle_on_real_colima() {
    let socket = socket();
    let name = format!("wharfkeep-test-{}", std::process::id());
    let _guard = Cleanup {
        socket: socket.clone(),
        kind: "container",
        name: name.clone(),
    };
    let engine = Engine::connect(&socket).unwrap();

    pull_image(&engine, "nginx:alpine").await;
    let id = engine
        .run(spec("nginx:alpine", &name, None, vec!["8080:80"]))
        .await
        .unwrap();
    let summary = engine.summary().await.unwrap();
    let row = summary
        .containers
        .iter()
        .find(|c| c.name == name)
        .expect("summary に出ない");
    assert_eq!(row.id, id);
    assert_eq!(row.state, "running");
    assert!(row.ports.contains("8080->80/tcp"), "{}", row.ports);
    assert!(!row.failed);
    assert!(
        summary
            .images
            .iter()
            .any(|i| i.repo_tags.iter().any(|t| t == "nginx:alpine"))
    );
    assert!(
        summary
            .networks
            .iter()
            .any(|n| n.name == "bridge" && n.builtin)
    );
    println!("disk = {:?}", summary.disk);
    assert!(summary.disk.images.is_some(), "df を読めていない");

    let usage = engine.stats_once(&id).await.unwrap();
    assert!(usage.mem_limit > 0);
    let value = engine.inspect(&id).await.unwrap();
    assert_eq!(value["Name"], format!("/{name}"));
    let mut logs = Box::pin(engine.logs(&id, false));
    while let Some(l) = logs.next().await {
        l.unwrap();
    }
    drop(logs);

    engine.stop(&id).await.unwrap();
    engine.remove(&id, true).await.unwrap();
    assert!(!has(&engine, &name).await, "消えていない");
}

#[tokio::test]
#[ignore = "実機の colima が要る"]
async fn run_failure_leaves_no_container() {
    let socket = socket();
    let name = format!("wharfkeep-test-{}-bad", std::process::id());
    let _guard = Cleanup {
        socket: socket.clone(),
        kind: "container",
        name: name.clone(),
    };
    let engine = Engine::connect(&socket).unwrap();
    // イメージがないと作成の段階で失敗し、開始失敗の後始末を試せない。
    // nginx は ENTRYPOINT があり cmd が引数になって開始できてしまうので、ENTRYPOINT のない alpine を使う。
    pull_image(&engine, "alpine:latest").await;

    let err = engine
        .run(spec(
            "alpine:latest",
            &name,
            Some(vec!["/nonexistent".into()]),
            vec![],
        ))
        .await
        .expect_err("開始に失敗するはず");
    println!("run error = {err}");
    assert!(
        err.to_string().contains("nonexistent"),
        "開始の失敗ではない: {err}"
    );
    assert!(!has(&engine, &name).await, "作りかけが残っている");
}

#[tokio::test]
#[ignore = "実機の colima が要る"]
async fn volume_and_network_lifecycle() {
    let socket = socket();
    let base = format!("wharfkeep-test-{}-res", std::process::id());
    let _volume = Cleanup {
        socket: socket.clone(),
        kind: "volume",
        name: base.clone(),
    };
    let _network = Cleanup {
        socket: socket.clone(),
        kind: "network",
        name: base.clone(),
    };
    let engine = Engine::connect(&socket).unwrap();

    engine.create_volume(&base).await.unwrap();
    engine
        .create_network(&base, NetDriver::Bridge)
        .await
        .unwrap();
    let summary = engine.summary().await.unwrap();
    let vol = summary
        .volumes
        .iter()
        .find(|v| v.name == base)
        .expect("volume");
    assert_eq!(vol.driver, "local");
    let net = summary
        .networks
        .iter()
        .find(|n| n.name == base)
        .expect("network");
    assert!(!net.builtin);
    assert_eq!(engine.inspect_volume(&base).await.unwrap()["Name"], base);
    assert_eq!(engine.inspect_network(&net.id).await.unwrap()["Name"], base);

    engine.remove_volume(&base).await.unwrap();
    engine.remove_network(&net.id).await.unwrap();
    let summary = engine.summary().await.unwrap();
    assert!(!summary.volumes.iter().any(|v| v.name == base));
    assert!(!summary.networks.iter().any(|n| n.name == base));
}

#[tokio::test]
#[ignore = "実機の colima が要る"]
async fn container_that_fails_to_start_is_flagged_failed() {
    let socket = socket();
    let name = format!("wharfkeep-start-fail-{}", std::process::id());
    let _guard = Cleanup {
        socket: socket.clone(),
        kind: "container",
        name: name.clone(),
    };
    let engine = Engine::connect(&socket).unwrap();
    pull_image(&engine, "alpine:latest").await;
    // 作成は成功し、起動で失敗する。コンテナは created のまま残る。
    let out = Command::new("docker")
        .arg("-H")
        .arg(format!("unix://{}", socket.display()))
        .args(["run", "--name", &name, "alpine:latest", "/nonexistent-cmd"])
        .output()
        .unwrap();
    assert!(!out.status.success());

    let summary = engine.summary().await.unwrap();
    let row = summary.containers.iter().find(|c| c.name == name).unwrap();
    assert_eq!(row.state, "created");
    assert!(row.failed);
}
