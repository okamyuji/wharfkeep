//! 試験用の偽UNIXソケット。要求の先頭行と本文を記録し、経路に合う応答を返す。

use crate::docker::Engine;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

/// 経路に`contains`を含む要求へ、`status`と`body`を返す。
pub struct Route {
    pub contains: &'static str,
    pub status: u16,
    pub body: &'static str,
}

pub fn route(contains: &'static str, body: &'static str) -> Route {
    Route {
        contains,
        status: 200,
        body,
    }
}

pub struct Fake {
    pub engine: Engine,
    seen: Arc<Mutex<Vec<String>>>,
    _dir: tempfile::TempDir,
}

impl Fake {
    /// 記録した要求。各項目は「メソッド 経路」と本文を改行でつないだもの。
    pub fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }
}

/// 要求を最後まで読む。ヘッダーの`Content-Length`の分だけ本文も読む。
async fn read_request(conn: &mut tokio::net::UnixStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = conn.read(&mut chunk).await.unwrap_or(0);
        buf.extend_from_slice(&chunk[..n]);
        let text = String::from_utf8_lossy(&buf).into_owned();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head
                .lines()
                .filter_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")?
                        .trim()
                        .parse::<usize>()
                        .ok()
                })
                .next()
                .unwrap_or(0);
            if body.len() >= length || n == 0 {
                let line = head.lines().next().unwrap_or_default();
                let mut parts = line.split_whitespace();
                let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                return format!("{method} {path}\n{body}");
            }
        } else if n == 0 {
            return text;
        }
    }
}

pub fn fake_docker(routes: Vec<Route>) -> Fake {
    let dir = tempfile::tempdir().unwrap();
    let sock = dir.path().join("docker.sock");
    let listener = UnixListener::bind(&sock).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let routes = Arc::new(routes);
    tokio::spawn(async move {
        while let Ok((mut conn, _)) = listener.accept().await {
            let (log, routes) = (log.clone(), routes.clone());
            tokio::spawn(async move {
                let request = read_request(&mut conn).await;
                let path = request.lines().next().unwrap_or_default().to_string();
                log.lock().unwrap().push(request);
                let (status, body) = routes
                    .iter()
                    .find(|r| path.contains(r.contains))
                    .map_or((404, "{}"), |r| (r.status, r.body));
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = conn.write_all(resp.as_bytes()).await;
            });
        }
    });
    Fake {
        engine: Engine::connect(&sock).unwrap(),
        seen,
        _dir: dir,
    }
}
