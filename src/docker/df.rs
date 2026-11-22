//! `GET /system/df`（Docker Engine API v1.52 以降の形式）から使用量を読む。

use super::{DockerError, size_to_option};
use crate::snapshot::DiskUsage;
use bollard::Docker;
use bollard::models::SystemDataUsageResponse;
use bollard::query_parameters::DataUsageOptionsBuilder;
use std::collections::HashMap;
use std::time::Duration;

/// df は重い。GUI が状態ファイルを古いと見なす 15 秒より十分に短く切る。
pub(super) const DF_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) type VolumeSizes = HashMap<String, Option<u64>>;

/// `limit` を超えたら `Err` を返す。
pub(super) async fn fetch(
    docker: &Docker,
    limit: Duration,
) -> Result<(DiskUsage, VolumeSizes), DockerError> {
    // verbose にしないとボリュームごとの `Items` が付かない。
    let opts = DataUsageOptionsBuilder::new().verbose(true).build();
    let resp = tokio::time::timeout(limit, docker.df(Some(opts)))
        .await
        .map_err(|_| DockerError::Api("system df timed out".into()))??;
    Ok(usage(&resp))
}

fn usage(resp: &SystemDataUsageResponse) -> (DiskUsage, VolumeSizes) {
    let total = |t: Option<i64>| t.and_then(size_to_option);
    let disk = DiskUsage {
        images: total(resp.image_usage.as_ref().and_then(|u| u.total_size)),
        containers: total(resp.container_usage.as_ref().and_then(|u| u.total_size)),
        volumes: total(resp.volume_usage.as_ref().and_then(|u| u.total_size)),
        build_cache: total(resp.build_cache_usage.as_ref().and_then(|u| u.total_size)),
    };
    let volumes = resp
        .volume_usage
        .iter()
        .flat_map(|u| u.items.iter().flatten())
        .filter_map(|v| {
            let name = v["Name"].as_str()?.to_string();
            let size = v["UsageData"]["Size"].as_i64().and_then(size_to_option);
            Some((name, size))
        })
        .collect();
    (disk, volumes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::API_DEFAULT_VERSION;
    use std::path::Path;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    const SAMPLE: &str = r#"{
        "ImageUsage": {"TotalSize": 1000, "TotalCount": 2},
        "ContainerUsage": {"TotalSize": 12},
        "VolumeUsage": {"TotalSize": 30, "Items": [
            {"Name": "a", "UsageData": {"Size": 30, "RefCount": 1}},
            {"Name": "b", "UsageData": {"Size": -1, "RefCount": -1}},
            {"Name": "c"},
            {"UsageData": {"Size": 9}}
        ]},
        "BuildCacheUsage": {"TotalSize": 123}
    }"#;

    fn parse(json: &str) -> (DiskUsage, VolumeSizes) {
        usage(&serde_json::from_str(json).unwrap())
    }

    #[test]
    fn parses_current_df_format() {
        let (disk, sizes) = parse(SAMPLE);
        assert_eq!(disk.images, Some(1000));
        assert_eq!(disk.containers, Some(12));
        assert_eq!(disk.volumes, Some(30));
        assert_eq!(disk.build_cache, Some(123));
        assert_eq!(sizes["a"], Some(30));
        assert_eq!(sizes["b"], None);
        assert_eq!(sizes["c"], None);
        assert_eq!(sizes.len(), 3);
    }

    #[test]
    fn missing_sections_are_unknown() {
        let (disk, sizes) = parse("{}");
        assert_eq!(disk, DiskUsage::default());
        assert!(sizes.is_empty());
    }

    #[test]
    fn negative_total_is_unknown_and_zero_is_zero() {
        let (disk, _) =
            parse(r#"{"ImageUsage":{"TotalSize":-1},"BuildCacheUsage":{"TotalSize":0}}"#);
        assert_eq!(disk.images, None);
        assert_eq!(disk.build_cache, Some(0));
    }

    /// `verbose=true` を付けた要求にだけ `Items` を返す。`hang` なら応答しない。
    fn serve(path: &Path, hang: bool) {
        let listener = UnixListener::bind(path).unwrap();
        tokio::spawn(async move {
            while let Ok((mut conn, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let n = conn.read(&mut buf).await.unwrap_or(0);
                    if hang {
                        std::future::pending::<()>().await;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let body = if req.contains("verbose=true") {
                        SAMPLE
                    } else {
                        r#"{"VolumeUsage":{"TotalSize":30}}"#
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = conn.write_all(resp.as_bytes()).await;
                });
            }
        });
    }

    fn docker(path: &Path) -> Docker {
        Docker::connect_with_unix(path.to_str().unwrap(), 120, API_DEFAULT_VERSION).unwrap()
    }

    #[tokio::test]
    async fn fetch_asks_for_verbose_and_reads_volume_sizes() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("d.sock");
        serve(&sock, false);
        let (disk, sizes) = fetch(&docker(&sock), DF_TIMEOUT).await.unwrap();
        assert_eq!(disk.images, Some(1000));
        assert_eq!(sizes["a"], Some(30));
    }

    #[tokio::test]
    async fn fetch_gives_up_after_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("d.sock");
        serve(&sock, true);
        let e = fetch(&docker(&sock), Duration::from_millis(100))
            .await
            .expect_err("timed out");
        assert!(e.to_string().contains("timed out"), "{e}");
    }

    #[test]
    fn df_timeout_stays_under_the_stale_threshold() {
        assert!(DF_TIMEOUT < crate::snapshot::STALE_AFTER);
    }
}
