//! tldr-pages: short descriptions plus practical examples for ~7000 commands.
//! <https://github.com/tldr-pages/tldr>

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const URL: &str = "https://github.com/tldr-pages/tldr/releases/latest/download/tldr-pages.en.zip";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Example {
    pub description: String,
    pub command: String,
}

#[derive(Debug)]
pub struct Page {
    /// File stem, e.g. `git-commit`.
    pub key: String,
    /// Title as written in the page, e.g. `git commit`.
    pub title: String,
    pub description: String,
    pub examples: Vec<Example>,
    /// "More information: <url>".
    pub url: Option<String>,
    /// "This command is an alias of `vim`."
    pub alias_of: Option<String>,
    /// "See also: `cheat`, `navi`."
    pub see_also: Vec<String>,
}

pub fn download(dest: &Path) -> Result<()> {
    let bytes = ureq::get(URL)
        .call()
        .context("downloading tldr pages")?
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_vec()?;
    let tmp = dest.with_extension("part");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, dest)?;
    Ok(())
}

/// Read all pages for `common` and the current OS from the downloaded zip.
pub fn load(zip_path: &Path) -> Result<Vec<Page>> {
    let file = std::fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut pages = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Platform-specific pages first so they win over `common`.
    for platform in [current_platform(), "common"] {
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i)?;
            let name = entry.name()?.into_owned();
            let Some(file) = name.strip_prefix(platform).and_then(|n| n.strip_prefix('/')) else {
                continue;
            };
            let Some(key) = file.strip_suffix(".md") else { continue };
            if !seen.insert(key.to_string()) {
                continue;
            }
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            if let Some(page) = parse(key, &text) {
                pages.push(page);
            }
        }
    }
    Ok(pages)
}

/// The raw markdown of one page and its platform, preferring the current OS,
/// then `common`, then any other platform (e.g. a macOS-only tool on Linux).
pub fn read_page(zip_path: &Path, key: &str) -> Result<Option<(String, String)>> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip_path)?)?;
    let mut platforms = vec![current_platform().to_string(), "common".to_string()];
    for i in 0..archive.len() {
        let name = archive.by_index(i)?.name()?.into_owned();
        if let Some((platform, file)) = name.split_once('/') {
            if file == format!("{key}.md") && !platforms.iter().any(|p| p == platform) {
                platforms.push(platform.to_string());
            }
        }
    }
    for platform in platforms {
        if let Ok(mut entry) = archive.by_name(&format!("{platform}/{key}.md")) {
            let mut text = String::new();
            entry.read_to_string(&mut text)?;
            return Ok(Some((platform, text)));
        }
    }
    Ok(None)
}

fn current_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "osx",
        "windows" => "windows",
        "freebsd" => "freebsd",
        _ => "linux",
    }
}

pub fn parse(key: &str, text: &str) -> Option<Page> {
    let mut title = None;
    let mut desc = Vec::new();
    let mut examples = Vec::new();
    let mut pending: Option<String> = None;
    let mut url = None;
    let mut alias_of = None;
    let mut see_also = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if let Some(t) = line.strip_prefix("# ") {
            title = Some(t.to_string());
        } else if let Some(d) = line.strip_prefix("> ") {
            if let Some(info) = d.strip_prefix("More information:") {
                url = info.split(['<', '>']).nth(1).map(str::to_string);
            } else {
                if let Some(rest) = d.strip_prefix("See also:") {
                    see_also = code_spans(rest);
                }
                if let Some(rest) = d.strip_prefix("This command is an alias of") {
                    alias_of = code_spans(rest).into_iter().next();
                }
                desc.push(strip_markdown(d));
            }
        } else if let Some(d) = line.strip_prefix("- ") {
            pending = Some(strip_markdown(d.trim_end_matches(':')));
        } else if line.starts_with('`') && line.ends_with('`') && line.len() > 1 {
            if let Some(description) = pending.take() {
                examples.push(Example { description, command: line[1..line.len() - 1].to_string() });
            }
        }
    }
    Some(Page { key: key.to_string(), title: title?, description: desc.join(" "), examples, url, alias_of, see_also })
}

/// Texts between backticks: "`cheat`, `navi`" -> ["cheat", "navi"].
fn code_spans(s: &str) -> Vec<String> {
    s.split('`').skip(1).step_by(2).map(str::to_string).collect()
}

fn strip_markdown(s: &str) -> String {
    s.replace('`', "")
}

/// A project name hidden in the docs URL, e.g. `ripgrep` from
/// `https://github.com/BurntSushi/ripgrep` -- often the package name, and what
/// people remember when the command is called `rg`.
pub fn project_name(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let mut parts = rest.split(['/', '#', '?']);
    let host = parts.next()?;
    if !["github.com", "gitlab.com", "codeberg.org", "sr.ht", "git.sr.ht", "crates.io", "pypi.org", "www.npmjs.com"]
        .contains(&host)
    {
        // A project's own site: imagemagick.org -> imagemagick (for `magick`).
        let labels: Vec<&str> = host.split('.').collect();
        let name = labels.len().checked_sub(2).map(|i| labels[i].to_lowercase())?;
        return (name.len() >= 3 && !GENERIC_HOSTS.contains(&name.as_str())).then_some(name);
    }
    // github.com/<owner>/<repo>, crates.io/crates/<name>, pypi.org/project/<name>, ...
    let segs: Vec<&str> = parts.filter(|s| !s.is_empty()).collect();
    let name = segs.get(1)?.trim_end_matches(".git").to_lowercase();
    (!name.is_empty() && name != "tldr").then_some(name)
}

/// Sites that host docs for many projects, so their name says nothing.
const GENERIC_HOSTS: &[&str] = &[
    "manned", "gnu", "github", "gitlab", "microsoft", "freedesktop", "sourceforge", "npmjs", "amazon", "google",
    "apple", "archlinux", "debian", "ubuntu", "man7", "die", "readthedocs", "pypi", "crates", "rs", "golang",
    "mozilla", "kernel", "openbsd", "freebsd", "netbsd", "fedoraproject", "redhat", "suse", "opensuse",
    "gentoo", "voidlinux", "alpinelinux", "nixos", "brew", "docs", "wikipedia", "keith", "salsa", "savannah",
    "nongnu", "proxmox", "oracle", "ibm", "hashicorp", "jetbrains", "gitea", "sr", "codeberg", "launchpad",
    "pkg", "go", "readme", "manpages", "linux", "unix", "bsd", "android", "cisco", "azure", "cloud",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_page() {
        let md = "# rg\n\n> Ripgrep, a search tool.\n> See also: `grep`, `ag`.\n> More information: <https://github.com/BurntSushi/ripgrep>.\n\n- Search for `pattern`:\n\n`rg {{pattern}}`\n";
        let p = parse("rg", md).unwrap();
        assert_eq!(p.title, "rg");
        assert_eq!(p.description, "Ripgrep, a search tool. See also: grep, ag.");
        assert_eq!(p.examples.len(), 1);
        assert_eq!(p.examples[0].description, "Search for pattern");
        assert_eq!(p.examples[0].command, "rg {{pattern}}");
        assert_eq!(p.url.as_deref(), Some("https://github.com/BurntSushi/ripgrep"));
        assert_eq!(p.see_also, ["grep", "ag"]);
        assert_eq!(project_name(p.url.as_deref().unwrap()).as_deref(), Some("ripgrep"));
    }

    #[test]
    fn parses_alias() {
        let p = parse("vi", "# vi\n\n> This command is an alias of `vim`.\n").unwrap();
        assert_eq!(p.alias_of.as_deref(), Some("vim"));
    }

    #[test]
    fn project_names() {
        assert_eq!(project_name("https://crates.io/crates/tealdeer").as_deref(), Some("tealdeer"));
        assert_eq!(project_name("https://manned.org/grep"), None);
        assert_eq!(project_name("https://imagemagick.org/script/magick.php").as_deref(), Some("imagemagick"));
        assert_eq!(project_name("https://www.gnu.org/software/tar"), None);
        assert_eq!(project_name("https://github.com/tldr-pages/tldr/blob/main/x.md"), None);
    }
}
