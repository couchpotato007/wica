//! Terminal output.

use std::io::IsTerminal;

use std::collections::HashMap;

use crate::index::Tool;
use crate::packages::{Found, Hint};
use crate::search::Hit;

pub struct Style {
    color: bool,
    width: usize,
}


impl Style {
    pub fn detect() -> Self {
        let tty = std::io::stdout().is_terminal();
        let color = tty && std::env::var_os("NO_COLOR").is_none();
        let width = terminal_size::terminal_size().map(|(w, _)| w.0 as usize).unwrap_or(100);
        Self { color, width }
    }

    pub fn paint_dim(&self, s: &str) -> String {
        self.paint("2", s)
    }

    fn paint(&self, code: &str, s: &str) -> String {
        if self.color { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
    }
}

pub fn hits(hits: &[Hit], style: &Style, examples: bool, install: &HashMap<String, Vec<Hint>>) {
    let name_width = hits.iter().map(|h| h.tool.name.chars().count()).max().unwrap_or(0).min(24);
    for hit in hits {
        let tool = hit.tool;
        let name = format!("{:<name_width$}", tool.name);
        let shown_width = name.chars().count();
        let name = if tool.installed { style.paint("1;32", &name) } else { style.paint("33", &name) };
        let marker = if tool.installed { "" } else { " (not installed)" };
        let avail = style.width.saturating_sub(shown_width + 3 + marker.len());
        let summary = truncate(tool.summary(), avail);
        println!(" {name}  {summary}{}", style.paint("2", marker));

        if examples {
            if let Some(ex) = hit.example {
                let indent = " ".repeat(name_width + 3);
                println!("{indent}{}", style.paint("2", &truncate(&ex.description, avail)));
                println!("{indent}{} {}", style.paint("2", "$"), command(&ex.command, style));
            }
        }
        if !tool.installed {
            install_lines(&tool.name, name_width + 3, install, style);
        }
    }
}

fn install_lines(name: &str, indent: usize, install: &HashMap<String, Vec<Hint>>, style: &Style) {
    let indent = " ".repeat(indent);
    let hints = install.get(name).map(Vec::as_slice).unwrap_or_default();
    for hint in hints {
        let note = if hint.guessed { style.paint("2", "  (package name guessed)") } else { String::new() };
        // Only name the manager when there's more than one to choose from.
        let label = if hints.len() > 1 { format!("install ({}):", hint.manager) } else { "install:".into() };
        println!("{indent}{} {}{note}", style.paint("2", &label), style.paint("1", &hint.install));
        if let Some(t) = &hint.try_cmd {
            println!("{indent}{} {}", style.paint("2", "try:    "), t);
        }
    }
}

/// One line per tool, for `wica name`: name, the other name that matched
/// (e.g. its package), and the description.
pub fn names(matches: &[(&Tool, Option<&str>)], style: &Style, install: &HashMap<String, Vec<Hint>>) {
    let name_width = matches.iter().map(|(t, _)| t.name.chars().count()).max().unwrap_or(0).min(24);
    for (tool, via) in matches {
        let name = format!("{:<name_width$}", tool.name);
        let name = if tool.installed { style.paint("1;32", &name) } else { style.paint("33", &name) };
        let via = via.map(|v| format!("({v}) ")).unwrap_or_default();
        let marker = if tool.installed { "" } else { " (not installed)" };
        let avail = style.width.saturating_sub(name_width + 3 + via.chars().count() + marker.len());
        println!(
            " {name}  {}{}{}",
            style.paint("35", &via),
            truncate(tool.summary(), avail),
            style.paint("2", marker)
        );
        // Compact: only install commands whose package name is confirmed.
        let indent = " ".repeat(name_width + 3);
        for hint in install.get(&tool.name).into_iter().flatten().filter(|h| !h.guessed && !tool.installed) {
            println!("{indent}{} {}", style.paint("2", "install:"), style.paint("1", &hint.install));
        }
    }
}

/// Packages found by a package manager search that wica has no docs for.
pub fn packages(found: &[(String, Found)], style: &Style, install: &HashMap<String, Vec<Hint>>) {
    let name_width = found.iter().map(|(_, f)| f.name.chars().count()).max().unwrap_or(0).min(24);
    for (manager, f) in found {
        let name = format!("{:<name_width$}", f.name);
        let label = format!(" ({manager} package)");
        let avail = style.width.saturating_sub(name_width + 3 + label.len());
        let desc = truncate(f.description.as_deref().unwrap_or(""), avail);
        println!(" {}  {desc}{}", style.paint("33", &name), style.paint("2", &label));
        let indent = " ".repeat(name_width + 3);
        for hint in install.get(&f.name).into_iter().flatten().filter(|h| &h.manager == manager) {
            println!("{indent}{} {}", style.paint("2", "install:"), style.paint("1", &hint.install));
        }
    }
}

/// A whole tldr page, like a tldr client prints it.
pub fn page(markdown: &str, platform: &str, style: &Style) {
    for line in markdown.lines() {
        let line = line.trim_end();
        if let Some(title) = line.strip_prefix("# ") {
            let note = if platform == "common" { String::new() } else { format!("  ({platform})") };
            println!("\n  {}{}\n", style.paint("1", title), style.paint("2", &note));
        } else if let Some(text) = line.strip_prefix("> ") {
            if let Some(info) = text.strip_prefix("More information:") {
                let url = info.split(['<', '>']).nth(1).unwrap_or(info.trim());
                println!("  {} {}", style.paint("2", "More information:"), style.paint("4", url));
            } else {
                println!("  {}", inline_code(text, style));
            }
        } else if let Some(text) = line.strip_prefix("- ") {
            println!("\n  {}", style.paint("32", &format!("- {}", text.replace('`', ""))));
        } else if line.starts_with('`') && line.ends_with('`') && line.len() > 1 {
            println!("    {}", command(&line[1..line.len() - 1], style));
        }
    }
    println!();
}

/// `See also: `cheat`` -> the command names highlighted.
fn inline_code(text: &str, style: &Style) -> String {
    text.split('`')
        .enumerate()
        .map(|(i, part)| if i % 2 == 1 { style.paint("36", part) } else { part.to_string() })
        .collect()
}

/// Render tldr placeholders: `{{path/to/file}}` -> underlined, and
/// `{{[-F|--fixed-strings]}}` -> the long option.
fn command(cmd: &str, style: &Style) -> String {
    let mut out = String::new();
    let mut rest = cmd;
    while let Some(start) = rest.find("{{") {
        out.push_str(&style.paint("36", &rest[..start]));
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            rest = after;
            break;
        };
        let inner = &after[..end];
        match inner.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            Some(alts) => out.push_str(&style.paint("36", alts.rsplit('|').next().unwrap_or(alts))),
            None => out.push_str(&style.paint("4;35", inner)),
        }
        rest = &after[end + 2..];
    }
    out.push_str(&style.paint("36", rest));
    out
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}
