use std::process::ExitCode;
use wharfkeep::lock::InstanceLock;
use wharfkeep::{daemon, gui, paths};

fn main() -> ExitCode {
    // .appをFinderから開くと引数なしで起動するので、既定はdaemonにする。
    let sub = std::env::args().nth(1);
    let (lock_name, run): (&str, fn() -> Result<(), String>) = match sub.as_deref() {
        None | Some("daemon") => ("daemon.lock", daemon::run),
        Some("gui") => ("gui.lock", || gui::run().map_err(|e| e.to_string())),
        Some(other) => {
            eprintln!(
                "wharfkeep: 不明なサブコマンドです: {other}（daemon か gui を指定してください）"
            );
            return ExitCode::from(2);
        }
    };
    let dir = paths::data_dir();
    let lock = std::fs::create_dir_all(&dir)
        .and_then(|()| InstanceLock::try_acquire(&dir.join(lock_name)));
    let _lock = match lock {
        Ok(Some(l)) => l,
        // すでに動いているので何もしない。
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wharfkeep: ロックを取れません（{}）: {e}", dir.display());
            return ExitCode::FAILURE;
        }
    };
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wharfkeep: {e}");
            ExitCode::FAILURE
        }
    }
}
