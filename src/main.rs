use std::process::ExitCode;
use wharfkeep::i18n::{Lang, Msg};
use wharfkeep::lock::InstanceLock;
use wharfkeep::{daemon, gui, paths};

fn main() -> ExitCode {
    // .appをFinderから開くと引数なしで起動するので、既定はdaemonにする。
    let lang = Lang::detect();
    let sub = std::env::args().nth(1);
    let (lock_name, run): (&str, fn() -> Result<(), String>) = match sub.as_deref() {
        None | Some("daemon") => ("daemon.lock", daemon::run),
        Some("gui") => ("gui.lock", || gui::run().map_err(|e| e.to_string())),
        Some(other) => {
            eprintln!("wharfkeep: {other}: {}", Msg::UnknownSubcommand.text(lang));
            return ExitCode::from(2);
        }
    };
    let Some(dir) = paths::data_dir() else {
        eprintln!("wharfkeep: {}", Msg::NoHomeDir.text(lang));
        return ExitCode::FAILURE;
    };
    let lock = std::fs::create_dir_all(&dir)
        .and_then(|()| InstanceLock::try_acquire(&dir.join(lock_name)));
    let _lock = match lock {
        Ok(Some(l)) => l,
        // すでに動いているので何もしない。
        Ok(None) => return ExitCode::SUCCESS,
        Err(e) => {
            eprintln!(
                "wharfkeep: {} ({}): {e}",
                Msg::LockFailed.text(lang),
                dir.display()
            );
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
