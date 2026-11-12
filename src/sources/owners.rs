//! Which package an installed executable belongs to, so `wica name tealdeer`
//! finds the `tldr` binary. Read straight from local package databases (no
//! package manager is run, so this stays fast).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Package name per executable name.
pub fn packages(exes: &HashMap<String, PathBuf>) -> HashMap<String, String> {
    let mut owners = HashMap::new();
    // Nix: the store path names the package, e.g.
    // /nix/store/<hash>-tealdeer-1.7.1/bin/tldr.
    for (name, path) in exes {
        if let Some(pkg) = std::fs::canonicalize(path).ok().as_deref().and_then(nix_package) {
            owners.insert(name.clone(), pkg);
        }
    }
    for (path, pkg) in pacman().into_iter().chain(dpkg()) {
        if let Some(name) = path.strip_prefix("usr/bin/").or_else(|| path.strip_prefix("bin/")) {
            if exes.contains_key(name) {
                owners.entry(name.to_string()).or_insert(pkg);
            }
        }
    }
    owners
}

fn nix_package(path: &Path) -> Option<String> {
    let entry = path.strip_prefix("/nix/store").ok()?.components().next()?;
    let (_hash, name_version) = entry.as_os_str().to_str()?.split_once('-')?;
    Some(strip_version(name_version).to_string())
}

/// `tealdeer-1.7.1` -> `tealdeer`, `python3.12-httpie-3.2` -> `python3.12-httpie`:
/// the version starts at the first `-` followed by a digit.
fn strip_version(s: &str) -> &str {
    s.char_indices()
        .find(|&(i, c)| c == '-' && s[i + 1..].starts_with(|d: char| d.is_ascii_digit()))
        .map_or(s, |(i, _)| &s[..i])
}

/// (file path without leading `/`, package) from pacman's local database.
fn pacman() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(dirs) = std::fs::read_dir("/var/lib/pacman/local") else { return out };
    for dir in dirs.flatten() {
        let Ok(desc) = std::fs::read_to_string(dir.path().join("desc")) else { continue };
        let Some(name) = desc.split("%NAME%\n").nth(1).and_then(|s| s.lines().next()) else { continue };
        let Ok(files) = std::fs::read_to_string(dir.path().join("files")) else { continue };
        for f in files.lines().filter(|l| l.starts_with("usr/bin/")) {
            out.push((f.to_string(), name.to_string()));
        }
    }
    out
}

/// Same from dpkg's file lists (Debian, Ubuntu).
fn dpkg() -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/var/lib/dpkg/info") else { return out };
    for e in entries.flatten() {
        let file = e.file_name().to_string_lossy().into_owned();
        let Some(pkg) = file.strip_suffix(".list") else { continue };
        let pkg = pkg.split(':').next().unwrap_or(pkg); // drop ":amd64"
        let Ok(list) = std::fs::read_to_string(e.path()) else { continue };
        for f in list.lines().filter_map(|l| l.strip_prefix('/')) {
            if f.starts_with("usr/bin/") || f.starts_with("bin/") {
                out.push((f.to_string(), pkg.to_string()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nix_store_paths() {
        let p = Path::new("/nix/store/abc123-tealdeer-1.7.1/bin/tldr");
        assert_eq!(nix_package(p).as_deref(), Some("tealdeer"));
        let p = Path::new("/nix/store/abc123-python3.12-httpie-3.2.4/bin/http");
        assert_eq!(nix_package(p).as_deref(), Some("python3.12-httpie"));
        assert_eq!(nix_package(Path::new("/usr/bin/tldr")), None);
    }
}
