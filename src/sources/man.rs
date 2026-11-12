//! Read the one-line description from the NAME section of man pages.
//!
//! This is what `whatis`/`apropos` use, but we parse the pages ourselves so it
//! works without man-db and on every distro (Nix, Debian, Arch, Fedora, ...).

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;

/// Sections with user-facing commands: 1 = user commands, 8 = admin commands.
const SECTIONS: &[&str] = &["man1", "man8"];

/// Map of command name -> description.
pub fn descriptions(path_dirs: &[PathBuf]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for root in man_roots(path_dirs) {
        for section in SECTIONS {
            let Ok(entries) = std::fs::read_dir(root.join(section)) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = page_name(&path) else { continue };
                if out.contains_key(&name) {
                    continue;
                }
                if let Some(desc) = read_page(&path).and_then(|t| parse_name_section(&t)) {
                    out.insert(name, desc);
                }
            }
        }
    }
    out
}

/// Where man pages live: `$MANPATH`, `<bin>/../share/man` for every `$PATH`
/// entry (covers Nix profiles, Homebrew, ~/.local), and the FHS defaults.
fn man_roots(path_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(mp) = std::env::var_os("MANPATH") {
        roots.extend(std::env::split_paths(&mp).filter(|p| !p.as_os_str().is_empty()));
    }
    for dir in path_dirs {
        if let Some(parent) = dir.parent() {
            roots.push(parent.join("share/man"));
            roots.push(parent.join("man"));
        }
    }
    roots.extend(["/usr/share/man", "/usr/local/share/man", "/run/current-system/sw/share/man"].map(PathBuf::from));

    let mut seen = HashSet::new();
    roots
        .into_iter()
        .filter(|r| r.is_dir())
        .filter(|r| seen.insert(r.canonicalize().unwrap_or_else(|_| r.clone())))
        .collect()
}

/// `rg.1.gz` -> `rg`, `git-commit.1` -> `git-commit`.
fn page_name(path: &Path) -> Option<String> {
    let file = path.file_name()?.to_str()?;
    let file = file.strip_suffix(".gz").unwrap_or(file);
    let (name, section) = file.rsplit_once('.')?;
    section.starts_with(['1', '8']).then(|| name.to_string())
}

/// Read only the head of the page; the NAME section is always near the top.
fn read_page(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let reader: Box<dyn Read> = if path.extension().is_some_and(|e| e == "gz") {
        Box::new(GzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut text = String::new();
    let mut in_name = false;
    for line in BufReader::new(reader.take(256 * 1024)).split(b'\n').take(400) {
        let line = String::from_utf8_lossy(&line.ok()?).into_owned();
        let is_heading = is_section_heading(&line);
        if in_name && is_heading {
            break;
        }
        if is_heading && heading_title(&line).eq_ignore_ascii_case("name") {
            in_name = true;
            continue;
        }
        if in_name {
            text.push_str(&line);
            text.push('\n');
        }
    }
    in_name.then_some(text)
}

fn is_section_heading(line: &str) -> bool {
    line.starts_with(".SH") || line.starts_with(".Sh")
}

fn heading_title(line: &str) -> &str {
    line[3..].trim().trim_matches('"')
}

/// Turn the roff NAME section into "description".
/// Handles both man(7) (`foo \- does things`) and mdoc(7) (`.Nm foo` / `.Nd does things`).
fn parse_name_section(section: &str) -> Option<String> {
    let mut plain = String::new();
    let mut mdoc_desc = None;
    for line in section.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(".Nd ") {
            mdoc_desc = Some(clean_roff(rest));
        } else if line.starts_with(".\\\"") || line.starts_with("'\\\"") {
            // comment
        } else if let Some(rest) = line.strip_prefix('.') {
            // A macro like `.B foo` or `.IR a b`: keep its arguments.
            if let Some((_, args)) = rest.split_once(' ') {
                plain.push_str(&args.replace('"', ""));
                plain.push(' ');
            }
        } else {
            plain.push_str(line);
            plain.push(' ');
        }
    }
    if let Some(d) = mdoc_desc {
        return non_empty(d);
    }
    let plain = clean_roff(&plain);
    let desc = plain
        .split_once(" - ")
        .or_else(|| plain.split_once(" -- "))
        .map(|(_, d)| d)?;
    non_empty(desc.to_string())
}

fn non_empty(s: String) -> Option<String> {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

/// Strip roff escapes like `\fB`, `\-`, `\(em`, `\&`.
fn clean_roff(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('f') => match chars.next() {
                Some('(') => {
                    chars.next();
                    chars.next();
                }
                Some('[') => skip_until(&mut chars, ']'),
                _ => {}
            },
            Some('(') => {
                let glyph: String = chars.by_ref().take(2).collect();
                match glyph.as_str() {
                    "em" | "en" | "mi" | "hy" => out.push('-'),
                    "aq" | "cq" | "oq" => out.push('\''),
                    "dq" | "lq" | "rq" => out.push('"'),
                    _ => {}
                }
            }
            Some('[') => skip_until(&mut chars, ']'),
            Some('*') => match chars.next() {
                Some('(') => {
                    chars.next();
                    chars.next();
                }
                Some('[') => skip_until(&mut chars, ']'),
                _ => {}
            },
            Some('-') => out.push('-'),
            Some(' ') | Some('~') => out.push(' '),
            Some('e') | Some('\\') => out.push('\\'),
            Some('&' | '%' | ':' | 'c' | '|' | '^' | '/' | ',') | None => {}
            Some(other) => out.push(other),
        }
    }
    out
}

fn skip_until(chars: &mut impl Iterator<Item = char>, end: char) {
    for c in chars.by_ref() {
        if c == end {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn man_style() {
        let s = ".B rg\n\\- recursively search the current directory for lines matching a pattern\n";
        assert_eq!(
            parse_name_section(s).unwrap(),
            "recursively search the current directory for lines matching a pattern"
        );
    }

    #[test]
    fn escapes() {
        let s = "\\fBgrep\\fR, egrep \\- print lines that match patterns\n";
        assert_eq!(parse_name_section(s).unwrap(), "print lines that match patterns");
    }

    #[test]
    fn mdoc_style() {
        let s = ".Nm ssh\n.Nd OpenSSH remote login client\n";
        assert_eq!(parse_name_section(s).unwrap(), "OpenSSH remote login client");
    }

    #[test]
    fn names() {
        assert_eq!(page_name(Path::new("/x/man1/rg.1.gz")).unwrap(), "rg");
        assert_eq!(page_name(Path::new("/x/man8/ip-link.8")).unwrap(), "ip-link");
        assert_eq!(page_name(Path::new("/x/man1/perl.1p")).unwrap(), "perl");
    }
}
