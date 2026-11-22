//! `GET /system/df` を直接読む。
//!
//! 実機の daemon は API 1.53 と 1.54 を指定しても旧形式（`LayersSize`、`Volumes[].UsageData`）を返す。
//! bollard 0.21.1 の型は新形式（`ImageUsage` など）を期待するため、読むと全部 `None` になる。

use super::size_to_option;
use crate::snapshot::DiskUsage;
use serde::Deserialize;
use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

const DF_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) type VolumeSizes = HashMap<String, Option<u64>>;

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct Df {
    layers_size: i64,
    containers: Vec<ContainerDf>,
    volumes: Vec<VolumeDf>,
    build_cache: Option<Vec<CacheDf>>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct ContainerDf {
    size_rw: Option<i64>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct VolumeDf {
    name: String,
    usage_data: Option<UsageData>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct UsageData {
    size: i64,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase", default)]
struct CacheDf {
    size: i64,
}

pub(super) async fn fetch(socket: &Path) -> io::Result<(DiskUsage, VolumeSizes)> {
    let raw = tokio::time::timeout(DF_TIMEOUT, get(socket))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "system df timed out"))??;
    let body = split_http_body(&raw)?;
    parse_df(body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// HTTP/1.0 で頼むと daemon は chunked を使わず、接続を閉じて本文の終わりを示す。
async fn get(socket: &Path) -> io::Result<Vec<u8>> {
    let mut stream = UnixStream::connect(socket).await?;
    stream
        .write_all(b"GET /system/df HTTP/1.0\r\nHost: docker\r\n\r\n")
        .await?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await?;
    Ok(raw)
}

fn split_http_body(raw: &[u8]) -> io::Result<&[u8]> {
    let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, m.to_string());
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| bad("incomplete HTTP response"))?;
    let head = String::from_utf8_lossy(&raw[..end]).to_ascii_lowercase();
    if !head.lines().next().is_some_and(|l| l.contains(" 200 ")) {
        return Err(bad("system df returned an error status"));
    }
    if head.contains("transfer-encoding: chunked") {
        return Err(bad("chunked response is not supported"));
    }
    Ok(&raw[end + 4..])
}

fn parse_df(body: &[u8]) -> Result<(DiskUsage, VolumeSizes), serde_json::Error> {
    let df: Df = serde_json::from_slice(body)?;
    let volume_sizes: VolumeSizes = df
        .volumes
        .iter()
        .map(|v| {
            (
                v.name.clone(),
                v.usage_data.as_ref().and_then(|u| size_to_option(u.size)),
            )
        })
        .collect();
    let disk = DiskUsage {
        images: size_to_option(df.layers_size),
        containers: sum_known(
            df.containers
                .iter()
                .map(|c| c.size_rw.and_then(size_to_option)),
        ),
        volumes: sum_known(volume_sizes.values().copied()),
        build_cache: df
            .build_cache
            .map(|c| c.iter().filter_map(|e| size_to_option(e.size)).sum()),
    };
    Ok((disk, volume_sizes))
}

/// 1 件も測れていない一覧だけ `None` にする。空の一覧は 0。
fn sum_known(sizes: impl Iterator<Item = Option<u64>>) -> Option<u64> {
    let all: Vec<_> = sizes.collect();
    if !all.is_empty() && all.iter().all(Option::is_none) {
        return None;
    }
    Some(all.into_iter().flatten().sum())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
        "LayersSize": 1000,
        "Containers": [{"SizeRw": 5}, {"SizeRw": 7}, {"SizeRw": -1}, {}],
        "Volumes": [
            {"Name": "a", "UsageData": {"Size": 30, "RefCount": 1}},
            {"Name": "b", "UsageData": {"Size": -1, "RefCount": -1}},
            {"Name": "c"}
        ],
        "BuildCache": [{"Size": 100}, {"Size": 23}]
    }"#;

    #[test]
    fn parses_legacy_df() {
        let (disk, sizes) = parse_df(SAMPLE.as_bytes()).unwrap();
        assert_eq!(disk.images, Some(1000));
        assert_eq!(disk.containers, Some(12));
        assert_eq!(disk.volumes, Some(30));
        assert_eq!(disk.build_cache, Some(123));
        assert_eq!(sizes["a"], Some(30));
        assert_eq!(sizes["b"], None);
        assert_eq!(sizes["c"], None);
    }

    #[test]
    fn missing_sections_become_none_or_zero() {
        let (disk, sizes) = parse_df(b"{}").unwrap();
        assert_eq!(disk.images, Some(0));
        assert_eq!(disk.containers, Some(0));
        assert_eq!(disk.volumes, Some(0));
        assert_eq!(disk.build_cache, None);
        assert!(sizes.is_empty());
    }

    #[test]
    fn volumes_with_no_known_size_are_unknown() {
        let (disk, _) = parse_df(br#"{"Volumes":[{"Name":"x"}]}"#).unwrap();
        assert_eq!(disk.volumes, None);
    }

    #[test]
    fn negative_layers_size_is_unknown() {
        let (disk, _) = parse_df(br#"{"LayersSize":-1}"#).unwrap();
        assert_eq!(disk.images, None);
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(parse_df(b"not json").is_err());
    }

    #[test]
    fn http_body_is_split_after_headers() {
        let ok = b"HTTP/1.0 200 OK\r\nContent-Type: x\r\n\r\n{\"a\":1}\r\n\r\n";
        assert_eq!(split_http_body(ok).unwrap(), b"{\"a\":1}\r\n\r\n");
    }

    #[test]
    fn http_errors_are_rejected() {
        let err = b"HTTP/1.0 500 Internal Server Error\r\n\r\n{\"message\":\"x\"}";
        assert!(split_http_body(err).is_err());
        assert!(split_http_body(b"HTTP/1.0 200 OK\r\nno end of headers").is_err());
        assert!(split_http_body(b"").is_err());
        let chunked =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nbody\r\n0\r\n\r\n";
        assert!(split_http_body(chunked).is_err());
    }
}
