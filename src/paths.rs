use std::path::{Path, PathBuf};

/// Finderから起動したアプリはシェルのPATHを受け継がないため、Homebrewの既定の場所も探す。
const EXTRA_DIRS: [&str; 2] = ["/opt/homebrew/bin", "/usr/local/bin"];

/// アプリのデータを置くディレクトリ（`~/Library/Application Support/work.okamyuji.wharfkeep`）。
/// ホームディレクトリが分からなければ`None`。
pub fn data_dir() -> Option<PathBuf> {
    // directoriesのProjectDirsと同じ場所。directoriesは依存にMPL-2.0（option-ext）を含むので使わない。
    std::env::home_dir().map(|h| h.join("Library/Application Support/work.okamyuji.wharfkeep"))
}

/// データ置き場を作り、権限を0700にする。
/// `DirBuilder`の`mode`は新規作成にしか効かないので、作成済みの場合は`set_permissions`で直す。
/// 既存のパスがsymlinkなら、参照先の権限を変える。
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// `path_env`（PATHの値）と既知の場所から実行ファイル`name`を探す。
pub fn find_binary(name: &str, path_env: &str) -> Option<PathBuf> {
    let extra: Vec<PathBuf> = EXTRA_DIRS.iter().map(PathBuf::from).collect();
    find_binary_in(name, path_env, &extra)
}

fn find_binary_in(name: &str, path_env: &str, extra: &[PathBuf]) -> Option<PathBuf> {
    // 相対の要素（空の要素を含む）は作業ディレクトリ次第で別物を指すので探さない。
    std::env::split_paths(path_env)
        .filter(|d| d.is_absolute())
        .chain(extra.iter().cloned())
        .map(|dir| dir.join(name))
        .find(|p| is_executable(p))
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn make_exe(dir: &Path, name: &str, mode: u32) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(mode)).unwrap();
        p
    }

    #[test]
    fn finds_binary_outside_path_env() {
        let dir = tempfile::tempdir().unwrap();
        let exe = make_exe(dir.path(), "colima", 0o755);
        assert_eq!(
            find_binary_in("colima", "", &[dir.path().to_path_buf()]),
            Some(exe)
        );
    }

    #[test]
    fn path_env_wins_over_extra_dirs() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let in_path = make_exe(a.path(), "colima", 0o755);
        make_exe(b.path(), "colima", 0o755);
        let path_env = format!("/nonexistent:{}", a.path().display());
        assert_eq!(
            find_binary_in("colima", &path_env, &[b.path().to_path_buf()]),
            Some(in_path)
        );
    }

    #[test]
    fn skips_non_executable_and_directories() {
        let dir = tempfile::tempdir().unwrap();
        make_exe(dir.path(), "colima", 0o644);
        std::fs::create_dir(dir.path().join("docker")).unwrap();
        let path_env = dir.path().display().to_string();
        assert_eq!(find_binary_in("colima", &path_env, &[]), None);
        assert_eq!(find_binary_in("docker", &path_env, &[]), None);
    }

    #[test]
    fn ignores_relative_path_entries() {
        // cargo testの作業ディレクトリはcrateの根なので、そこに置いた実行ファイルは相対の名前で見える。
        let dir = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap();
        make_exe(dir.path(), "colima", 0o755);
        let rel = dir.path().file_name().unwrap().to_str().unwrap();
        assert!(Path::new(rel).join("colima").is_file());
        assert_eq!(find_binary_in("colima", rel, &[]), None);
        assert_eq!(find_binary_in("colima", &format!(":{rel}:"), &[]), None);
    }

    #[test]
    fn data_dir_ends_with_bundle_id_under_application_support() {
        assert!(
            data_dir()
                .unwrap()
                .ends_with("Library/Application Support/work.okamyuji.wharfkeep")
        );
    }

    fn mode_of(p: &Path) -> u32 {
        p.metadata().unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn ensure_private_dir_creates_missing_dirs_as_0700() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("a/b");
        ensure_private_dir(&dir).unwrap();
        assert_eq!(mode_of(&dir), 0o700);
        assert_eq!(mode_of(&root.path().join("a")), 0o700);
    }

    #[test]
    fn ensure_private_dir_tightens_an_existing_0755_dir() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("data");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&dir).unwrap();
        assert_eq!(mode_of(&dir), 0o700);
    }

    #[test]
    fn ensure_private_dir_reports_an_uncreatable_path() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("file");
        std::fs::write(&file, "").unwrap();
        assert!(ensure_private_dir(&file.join("sub")).is_err());
    }
}
