//! User-defined tags: `wica tag rg grep search text`.
//!
//! Stored as plain text in `~/.config/wica/tags`, one tool per line:
//! `rg: grep search text`. Read at search time, so no reindex is needed.

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;

pub fn file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("wica/tags"))
}

pub fn load() -> HashMap<String, Vec<String>> {
    let mut tags: HashMap<String, Vec<String>> = HashMap::new();
    let Some(text) = file().and_then(|f| std::fs::read_to_string(f).ok()) else { return tags };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        if let Some((tool, words)) = line.split_once(':') {
            tags.entry(tool.trim().to_string())
                .or_default()
                .extend(words.split_whitespace().map(str::to_string));
        }
    }
    tags
}

pub fn add(tool: &str, words: &[String]) -> Result<PathBuf> {
    let path = file().ok_or_else(|| anyhow::anyhow!("no config directory"))?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    writeln!(f, "{tool}: {}", words.join(" "))?;
    Ok(path)
}
