use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::Path;

/// 二重起動を防ぐロック。値を持つ間はロックが続き、dropで外れる。
pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    /// ほかのプロセスが持っていれば`Ok(None)`を返す。
    pub fn try_acquire(path: &Path) -> Result<Option<InstanceLock>, io::Error> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(InstanceLock { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(e),
        }
    }
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
}
