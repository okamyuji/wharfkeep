//! 開発用の作業。`cargo xtask <作業>`で動かす。
mod bundle;
mod version;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "使い方: cargo xtask <bundle [--no-bump | --release] | e2e>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = repo_root().and_then(|root| match args.split_first() {
        Some((task, rest)) if task == "bundle" => bundle::run(&root, rest),
        _ => Err(USAGE.to_string()),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
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
