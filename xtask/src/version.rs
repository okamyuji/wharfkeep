//! 組み立てる.appの版の扱い。ファイルもプロセスも触らない。

/// `[package]`の`version = "X.Y.Z"`。依存の表の`version`は見ない。
pub fn package_version(toml: &str) -> Result<&str, String> {
    let v = package_lines(toml)
        .find_map(|(_, l)| version_value(l))
        .ok_or("Cargo.toml の [package] に version がありません")?;
    parse(v)?;
    Ok(v)
}

/// `[package]`の版の行だけを`next`に書き換える。
pub fn set_package_version(toml: &str, next: &str) -> Result<String, String> {
    let (at, _) = package_lines(toml)
        .find(|(_, l)| version_value(l).is_some())
        .ok_or("Cargo.toml の [package] に version がありません")?;
    let mut out: Vec<&str> = toml.split_inclusive('\n').collect();
    let old = out[at];
    let ending = &old[old.trim_end_matches(['\n', '\r']).len()..];
    let line = format!("version = \"{next}\"{ending}");
    out[at] = &line;
    Ok(out.concat())
}

/// patchを1つ上げる。
pub fn bump_patch(version: &str) -> Result<String, String> {
    let [x, y, z] = parse(version)?;
    let z = z.checked_add(1).ok_or("patch が大きすぎます")?;
    Ok(format!("{x}.{y}.{z}"))
}

/// どこから組んだかの印。
#[derive(Debug, PartialEq, Eq)]
pub enum Mark {
    /// CIのrelease。印を付けない。
    Release,
    /// このリポジトリのコミット（短い形）。`dirty`なら変更がある。
    Commit { rev: String, dirty: bool },
    /// このリポジトリのgitの作業ツリーの外。
    NoGit,
}

/// CFBundleShortVersionString。手元の組み立てはsemverのbuild metadataで組んだコミットを示す。
pub fn shown(version: &str, mark: &Mark) -> String {
    match mark {
        Mark::Release => version.to_string(),
        Mark::Commit { rev, dirty } => {
            let dirty = if *dirty { ".dirty" } else { "" };
            format!("{version}+local.{rev}{dirty}")
        }
        Mark::NoGit => format!("{version}+local.nogit"),
    }
}

/// 行の添字と中身。`[package]`の見出しの次から、次の見出しの前まで。
fn package_lines(toml: &str) -> impl Iterator<Item = (usize, &str)> {
    toml.split_inclusive('\n')
        .map(|l| l.trim_end_matches(['\n', '\r']))
        .enumerate()
        .skip_while(|(_, l)| l.trim() != "[package]")
        .skip(1)
        .take_while(|(_, l)| !l.trim_start().starts_with('['))
}

fn version_value(line: &str) -> Option<&str> {
    line.strip_prefix("version = \"")?.strip_suffix('"')
}

fn parse(v: &str) -> Result<[u64; 3], String> {
    let bad = || format!("Cargo.toml の版 '{v}' が X.Y.Z の形ではありません");
    let parts: Vec<&str> = v.split('.').collect();
    let [x, y, z] = parts.as_slice() else {
        return Err(bad());
    };
    let num = |s: &str| {
        if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        s.parse::<u64>().map_err(|_| bad())
    };
    Ok([num(x)?, num(y)?, num(z)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOML: &str = "[workspace]\nmembers = [\".\"]\n\n[package]\nname = \"w\"\nversion = \"0.1.9\"\n\n[dependencies.x]\nversion = \"0.1.9\"\n";

    #[test]
    fn package_version_reads_only_the_package_table() {
        assert_eq!(package_version(TOML), Ok("0.1.9"));
        let dep_first = "[dependencies.x]\nversion = \"9.9.9\"\n[package]\nversion = \"0.2.0\"\n";
        assert_eq!(package_version(dep_first), Ok("0.2.0"));
    }

    #[test]
    fn package_version_refuses_a_missing_or_malformed_version() {
        assert!(package_version("[package]\nname = \"w\"\n[x]\nversion = \"1.0.0\"\n").is_err());
        assert!(package_version("version = \"1.0.0\"\n").is_err());
        for bad in [
            "1.0",
            "1.0.0.0",
            "1.0.0-rc.1",
            "1.a.0",
            "1..0",
            "+1.0.0",
            "",
        ] {
            let toml = format!("[package]\nversion = \"{bad}\"\n");
            let e = package_version(&toml).unwrap_err();
            assert!(e.contains("X.Y.Z"), "{bad}: {e}");
        }
    }

    #[test]
    fn set_package_version_rewrites_only_the_package_line() {
        let out = set_package_version(TOML, "0.1.10").unwrap();
        assert_eq!(out, TOML.replacen("0.1.9", "0.1.10", 1));
        assert!(out.ends_with("[dependencies.x]\nversion = \"0.1.9\"\n"));
        assert!(set_package_version("[x]\nversion = \"1.0.0\"\n", "1.0.1").is_err());
    }

    #[test]
    fn set_package_version_keeps_the_line_ending() {
        let crlf = "[package]\r\nversion = \"1.0.0\"\r\nname = \"w\"\r\n";
        assert_eq!(
            set_package_version(crlf, "1.0.1").unwrap(),
            crlf.replace("1.0.0", "1.0.1")
        );
        let last = "[package]\nversion = \"1.0.0\"";
        assert_eq!(
            set_package_version(last, "1.0.1").unwrap(),
            "[package]\nversion = \"1.0.1\""
        );
    }

    #[test]
    fn bump_patch_raises_only_the_patch() {
        assert_eq!(bump_patch("0.1.9").unwrap(), "0.1.10");
        assert_eq!(bump_patch("1.2.0").unwrap(), "1.2.1");
        assert!(bump_patch("1.2").is_err());
        assert!(bump_patch(&format!("0.0.{}", u64::MAX)).is_err());
    }

    #[test]
    fn shown_marks_local_builds() {
        let commit = |dirty| Mark::Commit {
            rev: "abc1234".into(),
            dirty,
        };
        assert_eq!(shown("0.1.0", &Mark::Release), "0.1.0");
        assert_eq!(shown("0.1.0", &commit(false)), "0.1.0+local.abc1234");
        assert_eq!(shown("0.1.0", &commit(true)), "0.1.0+local.abc1234.dirty");
        assert_eq!(shown("0.1.0", &Mark::NoGit), "0.1.0+local.nogit");
    }
}
