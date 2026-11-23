//! `cargo xtask bundle`を、成果物だけを作る偽のcargoで実際に流す。
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FAKE_CARGO: &str = r#"#!/bin/sh
case "$1" in
  build) [ -z "$FAKE_CARGO_FAIL" ] || exit 3
    mkdir -p target/release && printf x > target/release/wharfkeep ;;
  update) sed -n 's/^version = "\(.*\)"$/lock \1/p' Cargo.toml | head -n 1 > Cargo.lock ;;
esac
"#;

// 依存の表にも行頭の version があり、版上げが [package] の行だけを変えるかを見る。
const TOML: &str = "[package]\nversion = \"0.1.0\"\n\n[dependencies.x]\nversion = \"0.1.0\"\n";

struct Sandbox {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
    repo: PathBuf,
}

/// 利用者の git の設定（設定ファイル、GIT_CONFIG_COUNT などの環境経由のもの、
/// ホームの既定の除外ファイル）と、git の hook が渡す環境変数を読ませない。
fn isolate(cmd: &mut Command) -> &mut Command {
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(k);
        }
    }
    // /var/empty は空で書けない。
    cmd.env_remove("GITHUB_ACTIONS")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", "/var/empty")
        .env("XDG_CONFIG_HOME", "/var/empty")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = isolate(&mut Command::new("git"))
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// `own_git` が偽なら、親のリポジトリの中に .git の無い写しを置く。
fn sandbox(own_git: bool) -> Sandbox {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    let cargo = bin.join("cargo");
    fs::write(&cargo, FAKE_CARGO).unwrap();
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).unwrap();

    let repo = if own_git {
        root.join("Repo")
    } else {
        git(&root, &["init", "-q"]);
        root.join("sub")
    };
    fs::create_dir_all(repo.join("assets")).unwrap();
    fs::create_dir_all(repo.join("xtask")).unwrap();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/Info.plist");
    fs::copy(src, repo.join("assets/Info.plist")).unwrap();
    fs::write(repo.join("Cargo.toml"), TOML).unwrap();
    fs::write(repo.join("Cargo.lock"), "lock 0.1.0\n").unwrap();
    fs::write(repo.join(".gitignore"), "target/\n").unwrap();
    if own_git {
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "t"]);
    }
    Sandbox {
        _tmp: tmp,
        root,
        bin,
        repo,
    }
}

impl Sandbox {
    /// `repo`をリポジトリの根として渡す。カレントディレクトリはsandboxの外の根にする。
    fn run_at(&self, repo: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
        let path = format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap());
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_xtask"));
        cmd.arg("bundle")
            .args(args)
            .current_dir(&self.root)
            .env("PATH", path)
            .env("CARGO_MANIFEST_DIR", repo.join("xtask"));
        isolate(&mut cmd).envs(env.iter().copied());
        cmd.output().expect("run xtask")
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let out = self.run_at(&self.repo, args, env);
        assert!(out.status.success(), "xtask bundle: {out:?}");
        out
    }

    fn shown(&self) -> String {
        let plist = self.repo.join("target/wharfkeep.app/Contents/Info.plist");
        let out = Command::new("/usr/libexec/PlistBuddy")
            .args(["-c", "Print :CFBundleShortVersionString"])
            .arg(&plist)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn version(&self) -> String {
        fs::read_to_string(self.repo.join("Cargo.toml")).unwrap()
    }

    fn head(&self) -> String {
        git(&self.repo, &["rev-parse", "--short", "HEAD"])
    }
}

#[test]
fn a_local_build_carries_the_commit_and_raises_the_version() {
    let s = sandbox(true);
    // カレントディレクトリに別のプロジェクトがあっても、そちらを組まず書き換えない。
    fs::write(
        s.root.join("Cargo.toml"),
        "[package]\nversion = \"9.9.9\"\n",
    )
    .unwrap();
    s.run(&[], &[]);
    assert_eq!(s.shown(), format!("0.1.0+local.{}", s.head()));
    let plist = s.repo.join("target/wharfkeep.app/Contents");
    let out = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleVersion"])
        .arg(plist.join("Info.plist"))
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "0.1.0");
    assert_eq!(fs::read(plist.join("MacOS/wharfkeep")).unwrap(), b"x");
    assert_eq!(s.version(), TOML.replacen("0.1.0", "0.1.1", 1));
    assert_eq!(
        fs::read_to_string(s.repo.join("Cargo.lock")).unwrap(),
        "lock 0.1.1\n"
    );
    assert_eq!(
        fs::read_to_string(s.root.join("Cargo.toml")).unwrap(),
        "[package]\nversion = \"9.9.9\"\n"
    );
    assert!(!s.root.join("target").exists());
}

#[test]
fn the_version_raise_alone_is_not_dirty_but_an_untracked_file_is() {
    let s = sandbox(true);
    s.run(&[], &[]);
    // 前の.appに残ったファイルは、組み直すと消える。
    let stray = s.repo.join("target/wharfkeep.app/stray");
    fs::write(&stray, "").unwrap();
    s.run(&["--no-bump"], &[]);
    assert!(!stray.exists());
    assert_eq!(s.shown(), format!("0.1.1+local.{}", s.head()));
    assert!(s.version().contains("version = \"0.1.1\""));

    // 未追跡を表示しない設定でも、未追跡のファイルは変更に数える。
    git(&s.repo, &["config", "status.showUntrackedFiles", "no"]);
    fs::write(s.repo.join("new.rs"), "").unwrap();
    s.run(&["--no-bump"], &[]);
    assert_eq!(s.shown(), format!("0.1.1+local.{}.dirty", s.head()));
}

#[test]
fn the_release_mark_is_refused_outside_github_actions_before_anything_is_built() {
    let s = sandbox(true);
    // 前の組み立てを置き、止まったときに消さないことも見る。
    s.run(&["--no-bump"], &[]);
    let before = s.shown();
    let out = s.run_at(&s.repo, &["--release"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--release"));
    assert_eq!(s.shown(), before);

    s.run(&["--release"], &[("GITHUB_ACTIONS", "true")]);
    assert_eq!(s.shown(), "0.1.0");
    assert_eq!(s.version(), TOML);
}

#[test]
fn a_malformed_version_stops_before_building() {
    let s = sandbox(true);
    fs::write(s.repo.join("Cargo.toml"), "[package]\nversion = \"0.1\"\n").unwrap();
    let out = s.run_at(&s.repo, &[], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("X.Y.Z"));
    assert!(!s.repo.join("target").exists());
}

#[test]
fn unknown_or_conflicting_flags_are_refused() {
    let s = sandbox(true);
    for args in [&["--bogus"][..], &["--no-bump", "--release"]] {
        let out = s.run_at(&s.repo, args, &[("GITHUB_ACTIONS", "true")]);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(!s.repo.join("target").exists(), "{args:?}");
    }
}

#[test]
fn a_copy_inside_another_repository_is_marked_nogit() {
    let s = sandbox(false);
    s.run(&["--no-bump"], &[]);
    assert_eq!(s.shown(), "0.1.0+local.nogit");
}

#[test]
fn a_case_different_path_keeps_the_commit_mark() {
    let s = sandbox(true);
    let other = s.repo.with_file_name("REPO");
    // 大文字小文字を区別するファイルシステムでは同じ場所を指さないので確かめない。
    if !other.exists() {
        return;
    }
    let out = s.run_at(&other, &["--no-bump"], &[]);
    assert!(out.status.success(), "xtask bundle: {out:?}");
    assert_eq!(s.shown(), format!("0.1.0+local.{}", s.head()));
}

#[test]
fn a_git_dir_from_outside_is_ignored() {
    let s = sandbox(true);
    let other = sandbox(true);
    // 同じ中身を同じ秒にコミットすると同じハッシュになるので、別のコミットにする。
    git(
        &other.repo,
        &["commit", "-q", "--allow-empty", "-m", "other"],
    );
    assert_ne!(other.head(), s.head());
    let git_dir = other.repo.join(".git");
    s.run(&["--no-bump"], &[("GIT_DIR", git_dir.to_str().unwrap())]);
    assert_eq!(s.shown(), format!("0.1.0+local.{}", s.head()));
}

#[test]
fn a_work_tree_from_outside_is_ignored() {
    let s = sandbox(true);
    let other = sandbox(true);
    s.run(
        &["--no-bump"],
        &[("GIT_WORK_TREE", other.repo.to_str().unwrap())],
    );
    assert_eq!(s.shown(), format!("0.1.0+local.{}", s.head()));
}

#[test]
fn a_failed_build_reports_cargo_and_keeps_the_version() {
    let s = sandbox(true);
    let out = s.run_at(&s.repo, &[], &[("FAKE_CARGO_FAIL", "1")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("cargo が失敗しました"));
    assert_eq!(s.version(), TOML);
}

#[test]
fn a_missing_manifest_names_the_file() {
    let s = sandbox(true);
    fs::remove_file(s.repo.join("Cargo.toml")).unwrap();
    let out = s.run_at(&s.repo, &[], &[]);
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains(&format!("{}", s.repo.join("Cargo.toml").display())),
        "{err}"
    );
}
