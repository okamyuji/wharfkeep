//! リリースビルドを`target/wharfkeep.app`に組む。
//! Cargo.tomlの版は「次に組む.appの版」。組めたらpatchを1つ上げ、次のビルドに備える。
use crate::version::{self, Mark};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::{Command, Stdio};

const PLIST_BUDDY: &str = "/usr/libexec/PlistBuddy";

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    /// 組んだあとで版を上げる。
    Bump,
    /// 印は付けるが版は上げない。e2eの前など。
    NoBump,
    /// 印のない版を名乗る。CIのreleaseだけが使う。
    Release,
}

fn parse_args(args: &[String]) -> Result<Mode, String> {
    match args {
        [] => Ok(Mode::Bump),
        [a] if a == "--no-bump" => Ok(Mode::NoBump),
        [a] if a == "--release" => Ok(Mode::Release),
        _ => Err(format!(
            "bundle の引数 {args:?} が分かりません（--no-bump か --release を1つだけ）"
        )),
    }
}

pub fn run(root: &Path, args: &[String]) -> Result<(), String> {
    let mode = parse_args(args)?;
    let toml_path = root.join("Cargo.toml");
    let toml = fs::read_to_string(&toml_path).map_err(|e| io_err(&toml_path, e))?;
    let version = version::package_version(&toml)?;
    let mark = if mode == Mode::Release {
        // 印のない版を名乗るのはCIのreleaseだけにする。手元の.appと同じ番号のreleaseを見分けるため。
        if std::env::var_os("GITHUB_ACTIONS").is_none_or(|v| v.is_empty()) {
            return Err("bundle: --release は CI の release だけが使います".into());
        }
        Mark::Release
    } else {
        git_mark(root)
    };
    let shown = version::shown(version, &mark);
    cargo(root, &["build", "--release", "--locked"])?;
    let app = root.join("target/wharfkeep.app");
    assemble(root, &app, &shown, version).map_err(|e| format!("{}: {e}", app.display()))?;
    if mode != Mode::Bump {
        println!("{} ({shown})", app.display());
        return Ok(());
    }
    let next = version::bump_patch(version)?;
    let raised = version::set_package_version(&toml, &next)?;
    fs::write(&toml_path, raised).map_err(|e| io_err(&toml_path, e))?;
    // CIは--lockedで試験するので、Cargo.lockのwharfkeepの版も合わせる。
    cargo(root, &["update", "--workspace", "--offline", "--quiet"])?;
    println!("{} ({shown}, next {next})", app.display());
    Ok(())
}

/// このディレクトリ自身のリポジトリのコミット。外から渡された`GIT_DIR`や親のリポジトリは読まない。
fn git_mark(root: &Path) -> Mark {
    let top = git(root, &["rev-parse", "--show-toplevel"]);
    // 文字列でなく同じディレクトリかで比べる。macOSは大文字小文字の違うパスでも同じ場所を指す。
    if !top.is_some_and(|t| same_dir(Path::new(&t), root)) {
        return Mark::NoGit;
    }
    let rev = git(root, &["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    // 組んだあとの版上げだけでdirtyにしない。未追跡のファイルは、gitの設定によらず変更に数える。
    let status = git(
        root,
        &[
            "status",
            "--porcelain",
            "--untracked-files=normal",
            "--",
            ".",
            ":!Cargo.toml",
            ":!Cargo.lock",
        ],
    );
    Mark::Commit {
        rev,
        dirty: status.is_none_or(|s| !s.is_empty()),
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// 成功したときの標準出力（前後の空白を除く）。
fn git(root: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(k);
        }
    }
    let out = cmd
        .args(args)
        .current_dir(root)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn cargo(root: &Path, args: &[&str]) -> Result<(), String> {
    check(Command::new("cargo").args(args).current_dir(root))
}

fn assemble(root: &Path, app: &Path, shown: &str, version: &str) -> io::Result<()> {
    if app.exists() {
        fs::remove_dir_all(app)?;
    }
    fs::create_dir_all(app.join("Contents/MacOS"))?;
    let plist = app.join("Contents/Info.plist");
    fs::copy(root.join("assets/Info.plist"), &plist)?;
    for (key, value) in [
        ("CFBundleShortVersionString", shown),
        ("CFBundleVersion", version),
    ] {
        let set = format!("Set :{key} {value}");
        check(Command::new(PLIST_BUDDY).args(["-c", &set]).arg(&plist))
            .map_err(io::Error::other)?;
    }
    fs::copy(
        root.join("target/release/wharfkeep"),
        app.join("Contents/MacOS/wharfkeep"),
    )?;
    check(
        Command::new("plutil")
            .arg("-lint")
            .arg(&plist)
            .stdout(Stdio::null()),
    )
    .map_err(io::Error::other)
}

pub(crate) fn check(cmd: &mut Command) -> Result<(), String> {
    let name = cmd.get_program().to_os_string();
    let status = cmd.status().map_err(|e| format!("{}: {e}", show(&name)))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} が失敗しました（{status}）", show(&name)))
    }
}

fn show(s: &OsStr) -> String {
    s.to_string_lossy().into_owned()
}

pub(crate) fn io_err(path: &Path, e: io::Error) -> String {
    format!("{}: {e}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_args_picks_one_mode() {
        assert_eq!(parse_args(&[]), Ok(Mode::Bump));
        assert_eq!(parse_args(&args(&["--no-bump"])), Ok(Mode::NoBump));
        assert_eq!(parse_args(&args(&["--release"])), Ok(Mode::Release));
    }

    #[test]
    fn parse_args_refuses_unknown_and_combined_flags() {
        for bad in [
            &["--bogus"][..],
            &["--no-bump", "--release"],
            &["--release", "x"],
        ] {
            assert!(parse_args(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn same_dir_compares_the_directory_not_the_spelling() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a");
        fs::create_dir(&a).unwrap();
        assert!(same_dir(&a, &tmp.path().join("./a")));
        assert!(!same_dir(&a, tmp.path()));
        assert!(!same_dir(&a, &tmp.path().join("missing")));
    }
}
