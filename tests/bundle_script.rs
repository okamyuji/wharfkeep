//! scripts/bundle.sh を、成果物だけを作る偽の cargo で実際に流す。
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const FAKE_CARGO: &str = r#"#!/bin/sh
case "$1" in
  build) mkdir -p target/release && printf x > target/release/wharfkeep ;;
  update) sed -n 's/^version = "\(.*\)"$/lock \1/p' Cargo.toml | head -n 1 > Cargo.lock ;;
esac
"#;

struct Sandbox {
    _tmp: tempfile::TempDir,
    bin: PathBuf,
    repo: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
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
    let src = Path::new(env!("CARGO_MANIFEST_DIR"));
    fs::create_dir_all(repo.join("scripts")).unwrap();
    fs::create_dir_all(repo.join("assets")).unwrap();
    fs::copy(
        src.join("scripts/bundle.sh"),
        repo.join("scripts/bundle.sh"),
    )
    .unwrap();
    fs::copy(
        src.join("assets/Info.plist"),
        repo.join("assets/Info.plist"),
    )
    .unwrap();
    fs::write(repo.join("Cargo.toml"), "[package]\nversion = \"0.1.0\"\n").unwrap();
    fs::write(repo.join("Cargo.lock"), "lock 0.1.0\n").unwrap();
    fs::write(repo.join(".gitignore"), "target/\n").unwrap();
    if own_git {
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "t"]);
    }
    Sandbox {
        _tmp: tmp,
        bin,
        repo,
    }
}

impl Sandbox {
    fn run_in(&self, dir: &Path, env: &[(&str, &str)]) -> Output {
        let path = format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap());
        let mut cmd = Command::new("scripts/bundle.sh");
        // シェルで cd してから呼んだときと同じく、論理パスを PWD に渡す。
        cmd.current_dir(dir).env("PATH", path).env("PWD", dir);
        // CI の中で走っても、手元の組み立てとして試す。
        for k in [
            "GITHUB_ACTIONS",
            "WHARFKEEP_RELEASE",
            "WHARFKEEP_NO_BUMP",
            "GIT_DIR",
            "GIT_WORK_TREE",
        ] {
            cmd.env_remove(k);
        }
        cmd.envs(env.iter().copied());
        cmd.output().expect("run bundle.sh")
    }

    fn run(&self, env: &[(&str, &str)]) -> Output {
        let out = self.run_in(&self.repo, env);
        assert!(out.status.success(), "bundle.sh: {out:?}");
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
    s.run(&[]);
    assert_eq!(s.shown(), format!("0.1.0+local.{}", s.head()));
    assert!(s.version().contains("version = \"0.1.1\""));
    assert_eq!(
        fs::read_to_string(s.repo.join("Cargo.lock")).unwrap(),
        "lock 0.1.1\n"
    );
}

#[test]
fn the_version_raise_alone_is_not_dirty_but_an_untracked_file_is() {
    let s = sandbox(true);
    s.run(&[]);
    s.run(&[("WHARFKEEP_NO_BUMP", "1")]);
    assert_eq!(s.shown(), format!("0.1.1+local.{}", s.head()));
    assert!(s.version().contains("version = \"0.1.1\""));

    fs::write(s.repo.join("new.rs"), "").unwrap();
    s.run(&[("WHARFKEEP_NO_BUMP", "1")]);
    assert_eq!(s.shown(), format!("0.1.1+local.{}.dirty", s.head()));
}

#[test]
fn the_release_mark_is_refused_outside_github_actions_before_anything_is_built() {
    let s = sandbox(true);
    let out = s.run_in(&s.repo, &[("WHARFKEEP_RELEASE", "1")]);
    assert!(!out.status.success());
    assert!(!s.repo.join("target").exists());

    s.run(&[("WHARFKEEP_RELEASE", "1"), ("GITHUB_ACTIONS", "true")]);
    assert_eq!(s.shown(), "0.1.0");
    assert!(s.version().contains("version = \"0.1.0\""));
}

#[test]
fn a_copy_inside_another_repository_is_marked_nogit() {
    let s = sandbox(false);
    s.run(&[("WHARFKEEP_NO_BUMP", "1")]);
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
    let out = s.run_in(&other, &[("WHARFKEEP_NO_BUMP", "1")]);
    assert!(out.status.success(), "bundle.sh: {out:?}");
    assert_eq!(s.shown(), format!("0.1.0+local.{}", s.head()));
}
