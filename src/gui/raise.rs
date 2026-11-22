//! 2つ目の`gui`の起動が、動いているGUIのウィンドウを前面に出す。

use crate::lock::holder_pid;
use std::path::Path;

/// `gui.lock`を持つプロセスを前面に出す。pidを読めない、または前面に出せなければ`false`。
pub fn raise_running(lock_path: &Path) -> bool {
    raise_with(lock_path, activate_pid)
}

fn raise_with(lock_path: &Path, activate: impl FnOnce(u32) -> bool) -> bool {
    holder_pid(lock_path).is_some_and(activate)
}

/// `activateWithOptions`は別のプロセスのアプリを前面にする。`ActivateIgnoringOtherApps`はmacOS 14から
/// 効かず、`activateFromApplication`は相手が譲る側として呼ぶ必要があるので、どちらも使わない。
/// 前面になったかは非同期で、この戻り値は要求が通ったかどうかだけを示す。
#[cfg(target_os = "macos")]
fn activate_pid(pid: u32) -> bool {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    NSRunningApplication::runningApplicationWithProcessIdentifier(pid).is_some_and(|app| {
        app.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows)
    })
}

#[cfg(not(target_os = "macos"))]
fn activate_pid(_pid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn activates_the_pid_written_in_the_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui.lock");
        std::fs::write(&path, "4242").unwrap();
        let seen = Cell::new(0);
        assert!(raise_with(&path, |pid| {
            seen.set(pid);
            true
        }));
        assert_eq!(seen.get(), 4242);
    }

    #[test]
    fn reports_failure_when_activation_fails() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui.lock");
        std::fs::write(&path, "4242").unwrap();
        assert!(!raise_with(&path, |_| false));
    }

    #[test]
    fn does_not_activate_without_a_pid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui.lock");
        let called = Cell::new(false);
        assert!(!raise_with(&path, |_| {
            called.set(true);
            true
        }));
        assert!(!called.get());
    }
}
