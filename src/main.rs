use std::process::ExitCode;
use wharfkeep::i18n::{Lang, Msg};
use wharfkeep::lock::InstanceLock;
use wharfkeep::{daemon, gui, paths};

type Entry = fn() -> Result<(), String>;

fn main() -> ExitCode {
    let lang = Lang::detect();
    let sub = std::env::args().nth(1);
    let Some((lock_name, run)) = select(sub.as_deref()) else {
        let other = sub.unwrap_or_default();
        eprintln!("wharfkeep: {other}: {}", Msg::UnknownSubcommand.text(lang));
        return ExitCode::from(2);
    };
    let Some(dir) = paths::data_dir() else {
        eprintln!("wharfkeep: {}", Msg::NoHomeDir.text(lang));
        return ExitCode::FAILURE;
    };
    let lock_path = dir.join(lock_name);
    match paths::ensure_private_dir(&dir).and_then(|()| InstanceLock::try_acquire(&lock_path)) {
        Ok(Some(_lock)) => finish(run()),
        Ok(None) => already_running(
            sub.as_deref() == Some("gui"),
            || gui::raise_running(&lock_path),
            lang,
        ),
        Err(e) => {
            eprintln!(
                "wharfkeep: {} ({}): {e}",
                Msg::LockFailed.text(lang),
                dir.display()
            );
            ExitCode::FAILURE
        }
    }
}

/// サブコマンドから、取るロックの名前と本体を選ぶ。知らないサブコマンドならNone。
fn select(sub: Option<&str>) -> Option<(&'static str, Entry)> {
    match sub {
        // .appをFinderから開くと引数なしで起動するので、既定はdaemonにする。
        None | Some("daemon") => Some(("daemon.lock", daemon::run)),
        Some("gui") => Some(("gui.lock", || gui::run().map_err(|e| e.to_string()))),
        Some(_) => None,
    }
}

/// すでに動いている。GUIなら、開いているウィンドウを前面に出して終わる。
fn already_running(gui: bool, raise: impl FnOnce() -> bool, lang: Lang) -> ExitCode {
    if !gui || raise() {
        return ExitCode::SUCCESS;
    }
    eprintln!("wharfkeep: {}", Msg::RaiseFailed.text(lang));
    ExitCode::FAILURE
}

fn finish(r: Result<(), String>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("wharfkeep: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock_of(arg: Option<&str>) -> Option<&'static str> {
        select(arg).map(|(lock, _)| lock)
    }

    #[test]
    fn select_defaults_to_the_daemon() {
        assert_eq!(lock_of(None), Some("daemon.lock"));
        assert_eq!(lock_of(Some("daemon")), Some("daemon.lock"));
    }

    #[test]
    fn select_picks_the_gui_lock_for_gui() {
        assert_eq!(lock_of(Some("gui")), Some("gui.lock"));
    }

    #[test]
    fn select_refuses_an_unknown_subcommand() {
        assert_eq!(lock_of(Some("bogus")), None);
        assert_eq!(lock_of(Some("")), None);
    }

    #[test]
    fn a_running_daemon_ends_quietly_without_raising() {
        let code = already_running(false, || panic!("must not raise"), Lang::En);
        assert_eq!(code, ExitCode::SUCCESS);
    }

    #[test]
    fn a_running_gui_is_raised() {
        assert_eq!(already_running(true, || true, Lang::En), ExitCode::SUCCESS);
    }

    #[test]
    fn a_gui_that_cannot_be_raised_fails() {
        assert_eq!(already_running(true, || false, Lang::En), ExitCode::FAILURE);
    }

    #[test]
    fn finish_maps_the_result_to_the_exit_code() {
        assert_eq!(finish(Ok(())), ExitCode::SUCCESS);
        assert_eq!(finish(Err("boom".into())), ExitCode::FAILURE);
    }
}
