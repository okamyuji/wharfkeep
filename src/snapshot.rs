//! デーモンが書き、GUI が読む状態ファイル。書き手はデーモンだけ。

use crate::colima::Instance;
use crate::i18n::{Lang, Msg};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// これより古い状態は、デーモンが止まっているとみなす。
const STALE_AFTER: Duration = Duration::from_secs(15);

/// これを超える状態ファイルは読まない。実際の状態は数百 KiB までなので十分に大きい。
const MAX_READ_BYTES: u64 = 16 * 1024 * 1024;

/// 書き込み中に SIGKILL された一時ファイルを、書き込み中のものと見分ける猶予。
const TEMP_STALE_AFTER: Duration = Duration::from_secs(60);

static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub updated_at: SystemTime,
    #[serde(default)]
    pub instances: Vec<Instance>,
    /// キーはプロファイル名。
    #[serde(default)]
    pub docker: BTreeMap<String, DockerState>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DockerState {
    Ready(DockerSummary),
    Unavailable { reason: String },
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DockerSummary {
    #[serde(default)]
    pub containers: Vec<ContainerRow>,
    #[serde(default)]
    pub images: Vec<ImageRow>,
    #[serde(default)]
    pub volumes: Vec<VolumeRow>,
    #[serde(default)]
    pub networks: Vec<NetworkRow>,
    #[serde(default)]
    pub disk: DiskUsage,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ContainerRow {
    pub id: String,
    pub name: String,
    pub image: String,
    pub state: String,
    pub status: String,
    pub ports: String,
    /// 起動に失敗したコンテナ。メニューバーのアイコンを珊瑚色にする条件。
    pub failed: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ImageRow {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub size_bytes: u64,
    pub created: i64,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct VolumeRow {
    pub name: String,
    pub driver: String,
    pub mountpoint: String,
    /// `None` は取れなかった値で、画面では「不明」と出す。
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkRow {
    pub id: String,
    pub name: String,
    pub driver: String,
    /// 既定の bridge・host・none。削除できない。
    pub builtin: bool,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiskUsage {
    pub images: Option<u64>,
    pub containers: Option<u64>,
    pub volumes: Option<u64>,
    pub build_cache: Option<u64>,
}

#[derive(Debug)]
pub enum SnapshotError {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    TooLarge {
        path: PathBuf,
    },
}

impl SnapshotError {
    pub fn message(&self, lang: Lang) -> String {
        match self {
            Self::Io { path, source } => {
                format!(
                    "{}: {}: {source}",
                    Msg::SnapshotIo.text(lang),
                    path.display()
                )
            }
            Self::TooLarge { path } => {
                format!("{}: {}", Msg::SnapshotTooLarge.text(lang), path.display())
            }
            Self::Parse { path, source } => {
                format!(
                    "{}: {}: {source}",
                    Msg::SnapshotParse.text(lang),
                    path.display()
                )
            }
        }
    }
}

/// 同じディレクトリの一時ファイルに書いて fsync し、rename で差し替える。
/// 親ディレクトリは作らない。
pub fn write_atomic(path: &Path, s: &Snapshot) -> io::Result<()> {
    let json = serde_json::to_vec(s).map_err(io::Error::other)?;
    // 同じ名前を二度使わないため pid と連番を入れる。
    let n = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.{n}.tmp", std::process::id()));
    let tmp = path.with_file_name(name);
    let result = write_synced(&tmp, &json).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn write_synced(tmp: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(tmp)?;
    f.write_all(bytes)?;
    f.sync_all()
}

/// SIGKILL で残った `state.json.*.tmp` のうち、`TEMP_STALE_AFTER` より古いものを消す。
/// 消した数を返す。1件を消せなくても続ける。デーモンの起動時に呼ぶ。
pub fn remove_stale_temps(dir: &Path) -> io::Result<usize> {
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with("state.json.") && name.ends_with(".tmp")) {
            continue;
        }
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| now.duration_since(m).ok());
        if age.is_some_and(|a| a > TEMP_STALE_AFTER) && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// ファイルがなければ `Ok(None)`。`MAX_READ_BYTES` を超えるファイルは読まない。
pub fn read(path: &Path) -> Result<Option<Snapshot>, SnapshotError> {
    let io_err = |source| SnapshotError::Io {
        path: path.to_path_buf(),
        source,
    };
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_err(e)),
    };
    let too_large = || SnapshotError::TooLarge {
        path: path.to_path_buf(),
    };
    if file.metadata().map_err(io_err)?.len() > MAX_READ_BYTES {
        return Err(too_large());
    }
    // 長さを測ったあとに伸びるファイルや、長さ 0 と報告されるファイルがあるので、読む量も縛る。
    let mut bytes = Vec::new();
    file.take(MAX_READ_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_err)?;
    if bytes.len() as u64 > MAX_READ_BYTES {
        return Err(too_large());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|source| SnapshotError::Parse {
            path: path.to_path_buf(),
            source,
        })
}

/// 時計のずれで `updated_at` が未来になったときは古くないとみなす。
pub fn is_stale(now: SystemTime, s: &Snapshot) -> bool {
    now.duration_since(s.updated_at)
        .is_ok_and(|age| age > STALE_AFTER)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::colima::InstanceStatus;
    use std::time::UNIX_EPOCH;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn container(i: usize) -> ContainerRow {
        ContainerRow {
            id: format!("c{i}"),
            name: format!("web-{i}"),
            image: "nginx:latest".into(),
            state: "running".into(),
            status: "Up 3 minutes".into(),
            ports: "0.0.0.0:8080->80/tcp".into(),
            failed: i.is_multiple_of(2),
        }
    }

    fn sample(containers: usize) -> Snapshot {
        let summary = DockerSummary {
            containers: (0..containers).map(container).collect(),
            images: vec![ImageRow {
                id: "i1".into(),
                repo_tags: vec!["nginx:latest".into()],
                size_bytes: 1_000,
                created: 1_700_000_000,
            }],
            volumes: vec![
                VolumeRow {
                    name: "v1".into(),
                    driver: "local".into(),
                    mountpoint: "/var/lib/docker/volumes/v1".into(),
                    size_bytes: Some(5),
                },
                VolumeRow {
                    name: "v2".into(),
                    driver: "local".into(),
                    mountpoint: "/m".into(),
                    size_bytes: None,
                },
            ],
            networks: vec![NetworkRow {
                id: "n1".into(),
                name: "bridge".into(),
                driver: "bridge".into(),
                builtin: true,
            }],
            disk: DiskUsage {
                images: Some(1),
                containers: None,
                volumes: Some(3),
                build_cache: Some(4),
            },
        };
        let mut docker = BTreeMap::new();
        docker.insert("default".to_string(), DockerState::Ready(summary));
        docker.insert(
            "dev".to_string(),
            DockerState::Unavailable {
                reason: "socket gone".into(),
            },
        );
        Snapshot {
            updated_at: at(1_000),
            instances: vec![Instance {
                name: "default".into(),
                status: InstanceStatus::Running,
                arch: "aarch64".into(),
                cpus: 2,
                memory_bytes: 2 << 30,
                disk_bytes: 60 << 30,
                runtime: "docker".into(),
            }],
            docker,
        }
    }

    #[test]
    fn roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let s = sample(3);
        write_atomic(&path, &s).unwrap();
        assert_eq!(read(&path).unwrap(), Some(s));
    }

    #[test]
    fn write_replaces_existing_file_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        write_atomic(&path, &sample(1)).unwrap();
        let second = sample(2);
        write_atomic(&path, &second).unwrap();
        assert_eq!(read(&path).unwrap(), Some(second));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn written_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        write_atomic(&path, &sample(1)).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn write_fails_when_parent_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-such-dir").join("state.json");
        let err = write_atomic(&path, &sample(1)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn failed_rename_removes_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::create_dir(&path).unwrap();
        assert!(write_atomic(&path, &sample(1)).is_err());
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["state.json"]);
    }

    #[test]
    fn missing_file_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read(&dir.path().join("nope.json")).unwrap().is_none());
    }

    #[test]
    fn invalid_json_is_parse_error_with_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, "{not json").unwrap();
        let err = read(&path).unwrap_err();
        assert!(matches!(err, SnapshotError::Parse { .. }));
        for lang in [Lang::En, Lang::Ja] {
            assert!(err.message(lang).contains("state.json"));
        }
    }

    #[test]
    fn other_io_error_is_io_error_with_path() {
        let dir = tempfile::tempdir().unwrap();
        // ディレクトリを読むと NotFound 以外の io エラーになる。
        let err = read(dir.path()).unwrap_err();
        assert!(matches!(err, SnapshotError::Io { .. }));
        for lang in [Lang::En, Lang::Ja] {
            let m = err.message(lang);
            assert!(m.contains(&dir.path().display().to_string()));
        }
        assert_ne!(err.message(Lang::En), err.message(Lang::Ja));
    }

    #[test]
    fn partial_file_still_parses_and_unknown_fields_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(
            &path,
            r#"{"updated_at":{"secs_since_epoch":7,"nanos_since_epoch":0},"future_field":[1,2]}"#,
        )
        .unwrap();
        let s = read(&path).unwrap().unwrap();
        assert_eq!(s.updated_at, at(7));
        assert!(s.instances.is_empty());
        assert!(s.docker.is_empty());

        std::fs::write(
            &path,
            r#"{"updated_at":{"secs_since_epoch":7,"nanos_since_epoch":0},
                "docker":{"p":{"Ready":{"extra":true}}}}"#,
        )
        .unwrap();
        let s = read(&path).unwrap().unwrap();
        assert_eq!(s.docker["p"], DockerState::Ready(DockerSummary::default()));
    }

    #[test]
    fn stale_after_15s() {
        let s = sample(0);
        let t = s.updated_at;
        assert!(!is_stale(t, &s));
        assert!(!is_stale(t + Duration::from_secs(15), &s));
        assert!(is_stale(t + Duration::from_millis(15_001), &s));
    }

    #[test]
    fn older_snapshot_with_fewer_row_fields_still_parses() {
        let mut v = serde_json::to_value(sample(0)).unwrap();
        v["docker"] = serde_json::json!({"p": {"Ready": {"containers": [
            {"id": "c1", "name": "n", "image": "i", "state": "running", "status": "Up"}
        ]}}});
        let s: Snapshot = serde_json::from_value(v).unwrap();
        let DockerState::Ready(d) = &s.docker["p"] else {
            panic!("not ready")
        };
        let c = &d.containers[0];
        assert!(!c.failed);
        assert_eq!(c.ports, "");
    }

    #[test]
    fn future_updated_at_is_not_stale() {
        let s = sample(0);
        assert!(!is_stale(s.updated_at - Duration::from_secs(60), &s));
    }

    #[test]
    fn reader_never_sees_partial_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let s = sample(300);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..1_000 {
                    write_atomic(&path, &s).unwrap();
                }
            });
            for _ in 0..1_000 {
                match read(&path) {
                    Ok(Some(got)) => assert_eq!(got, s),
                    Ok(None) => {}
                    Err(e) => panic!("partial read: {}", e.message(Lang::En)),
                }
            }
        });
    }

    #[test]
    fn read_refuses_files_over_16_mib_without_parsing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, vec![b' '; MAX_READ_BYTES as usize + 1]).unwrap();
        let err = read(&path).unwrap_err();
        assert!(matches!(err, SnapshotError::TooLarge { .. }), "{err:?}");
        for lang in [Lang::En, Lang::Ja] {
            assert!(err.message(lang).contains("state.json"));
        }
        assert_ne!(err.message(Lang::En), err.message(Lang::Ja));
    }

    #[test]
    fn read_accepts_a_file_of_exactly_16_mib() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let json = serde_json::to_vec(&sample(1)).unwrap();
        let mut bytes = json.clone();
        bytes.resize(MAX_READ_BYTES as usize, b' ');
        std::fs::write(&path, bytes).unwrap();
        assert_eq!(read(&path).unwrap(), Some(sample(1)));
    }

    #[test]
    fn read_caps_a_file_that_grows_after_the_size_check() {
        // /dev/zero は長さ 0 と報告されるが、無限に読める。take の上限が効くか確かめる。
        let err = read(Path::new("/dev/zero")).unwrap_err();
        assert!(matches!(err, SnapshotError::TooLarge { .. }), "{err:?}");
    }

    fn touch_aged(dir: &Path, name: &str, age: Duration) -> PathBuf {
        let p = dir.join(name);
        let f = std::fs::File::create(&p).unwrap();
        f.set_modified(SystemTime::now() - age).unwrap();
        p
    }

    #[test]
    fn remove_stale_temps_removes_only_old_state_json_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let secs = Duration::from_secs;
        let old = touch_aged(dir.path(), "state.json.123.4.tmp", secs(120));
        let fresh = touch_aged(dir.path(), "state.json.123.5.tmp", secs(5));
        let edge = touch_aged(dir.path(), "state.json.123.6.tmp", secs(55));
        let real = touch_aged(dir.path(), "state.json", secs(600));
        let other = touch_aged(dir.path(), "other.json.1.2.tmp", secs(600));
        let nontmp = touch_aged(dir.path(), "state.json.1.2.bak", secs(600));
        assert_eq!(remove_stale_temps(dir.path()).unwrap(), 1);
        assert!(!old.exists());
        for p in [fresh, edge, real, other, nontmp] {
            assert!(p.exists(), "{p:?}");
        }
    }

    #[test]
    fn remove_stale_temps_on_missing_dir_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(remove_stale_temps(&dir.path().join("none")).is_err());
    }

    #[test]
    fn remove_stale_temps_ignores_entries_it_cannot_remove() {
        let dir = tempfile::tempdir().unwrap();
        let stuck = dir.path().join("state.json.1.2.tmp");
        std::fs::create_dir(&stuck).unwrap();
        std::fs::File::open(&stuck)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(600))
            .unwrap();
        assert_eq!(remove_stale_temps(dir.path()).unwrap(), 0);
    }
}
