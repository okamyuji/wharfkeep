//! 開発用の作業。`cargo xtask <作業>`で動かす。
mod bundle;
mod install;
mod version;
mod wait;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str =
    "使い方: cargo xtask <bundle [--no-bump | --release] | install [--local] | uninstall | e2e>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = repo_root().and_then(|root| {
        let (name, rest) = args.split_first().ok_or(USAGE)?;
        match task_of(name).ok_or(USAGE)? {
            Task::Bundle => bundle::run(&root, rest),
            Task::Install => install::run_install(&root, rest),
            Task::Uninstall => install::run_uninstall(rest),
        }
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Task {
    Bundle,
    Install,
    Uninstall,
}

/// 作業の名前から選ぶ。試験から実機を書き換える作業へ流れないよう、条件でなく文字列のパターンで選ぶ。
fn task_of(name: &str) -> Option<Task> {
    match name {
        "bundle" => Some(Task::Bundle),
        "install" => Some(Task::Install),
        "uninstall" => Some(Task::Uninstall),
        _ => None,
    }
}

/// `cargo run`が渡す`CARGO_MANIFEST_DIR`（xtask/）の親。カレントディレクトリにもCDPATHにも頼らない。
fn repo_root() -> Result<PathBuf, String> {
    let dir = std::env::var_os("CARGO_MANIFEST_DIR")
        .ok_or("CARGO_MANIFEST_DIR がありません。cargo xtask から実行してください")?;
    PathBuf::from(dir)
        .parent()
        .map(PathBuf::from)
        .ok_or_else(|| "CARGO_MANIFEST_DIR に親がありません".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_of_knows_each_task_by_its_exact_name() {
        assert_eq!(task_of("bundle"), Some(Task::Bundle));
        assert_eq!(task_of("install"), Some(Task::Install));
        assert_eq!(task_of("uninstall"), Some(Task::Uninstall));
        for other in ["", "Bundle", "bogus", "install --local"] {
            assert_eq!(task_of(other), None, "{other}");
        }
    }
}
