use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, Write};
use std::path::Path;

/// 二重起動を防ぐロック。値を持つ間はロックが続き、dropで外れる。
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    /// ほかのプロセスが持っていれば`Ok(None)`を返す。
    pub fn try_acquire(path: &Path) -> Result<Option<InstanceLock>, io::Error> {
        use std::os::unix::fs::OpenOptionsExt;
        // 取ったあとに切り詰めるので、symlinkの先のファイルを書き換えないよう辿らない。
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => {
                // 後から来た起動が、動いているほうを見つけるために自分のpidを残す。
                file.set_len(0)?;
                (&file).write_all(std::process::id().to_string().as_bytes())?;
                Ok(Some(InstanceLock { _file: file }))
            }
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(e),
        }
    }
}

/// ロックを持つプロセスが書いたpid。ファイルがない、空、数字でないときは`None`。
pub fn holder_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.lock");
        let first = InstanceLock::try_acquire(&path).unwrap();
        assert!(first.is_some());
        assert!(InstanceLock::try_acquire(&path).unwrap().is_none());
    }

    #[test]
    fn released_after_drop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui.lock");
        drop(InstanceLock::try_acquire(&path).unwrap());
        assert!(InstanceLock::try_acquire(&path).unwrap().is_some());
    }

    #[test]
    fn missing_parent_dir_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("none").join("daemon.lock");
        assert!(InstanceLock::try_acquire(&path).is_err());
    }

    #[test]
    fn a_symlinked_lock_is_refused_and_its_target_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, "important data").unwrap();
        let path = dir.path().join("gui.lock");
        std::os::unix::fs::symlink(&victim, &path).unwrap();
        assert!(InstanceLock::try_acquire(&path).is_err());
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "important data");
    }

    #[test]
    fn holder_pid_is_the_pid_of_the_lock_owner() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui.lock");
        let _held = InstanceLock::try_acquire(&path).unwrap();
        assert_eq!(holder_pid(&path), Some(std::process::id()));
    }

    #[test]
    fn holder_pid_is_none_without_a_valid_pid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui.lock");
        assert_eq!(holder_pid(&path), None);
        std::fs::write(&path, "").unwrap();
        assert_eq!(holder_pid(&path), None);
        std::fs::write(&path, "not a pid").unwrap();
        assert_eq!(holder_pid(&path), None);
        std::fs::write(&path, " 4242\n").unwrap();
        assert_eq!(holder_pid(&path), Some(4242));
    }

    #[test]
    fn reacquiring_replaces_a_longer_stale_pid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui.lock");
        std::fs::write(&path, "99999999999999").unwrap();
        let _held = InstanceLock::try_acquire(&path).unwrap();
        assert_eq!(holder_pid(&path), Some(std::process::id()));
    }
}
