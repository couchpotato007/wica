//! Discover executables on `$PATH`.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

/// All executables on `$PATH`, keyed by name. The first match wins, same as the shell.
pub fn executables() -> HashMap<String, PathBuf> {
    let mut found = HashMap::new();
    for dir in path_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else { continue };
            if found.contains_key(&name) || name.starts_with('.') {
                continue;
            }
            // `metadata` follows symlinks, which is what we want on Nix and in /usr/bin.
            let Ok(meta) = std::fs::metadata(entry.path()) else { continue };
            if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
                found.insert(name, entry.path());
            }
        }
    }
    found
}

pub fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default()
}

/// Find an executable like `which`: a bare name is searched on `$PATH`,
/// a path is checked directly.
pub fn find(name: &str) -> Option<PathBuf> {
    let is_exe = |p: &std::path::Path| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if name.contains('/') {
        let p = PathBuf::from(name);
        return is_exe(&p).then_some(p);
    }
    path_dirs().into_iter().map(|d| d.join(name)).find(|p| is_exe(p))
}
