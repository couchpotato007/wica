//! The on-disk index: every known tool with its merged descriptions.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::embed::{self, Embedder};
use crate::sources::{man, owners, path, tldr};

const VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    /// Display name, e.g. `rg` or `git commit`.
    pub name: String,
    /// Whether the command (or its parent command) is on `$PATH`.
    pub installed: bool,
    pub tldr: Option<String>,
    pub man: Option<String>,
    pub examples: Vec<tldr::Example>,
    /// Package that provides the installed binary (`tealdeer` for `tldr`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// Docs URL from tldr ("More information").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// tldr says this is another name for that command (`vi` -> `vim`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias_of: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub see_also: Vec<String>,
}

impl Tool {
    /// Other names people may know this tool by: its package and the project
    /// name in its docs URL (`ripgrep` for `rg`).
    pub fn other_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.package.iter().cloned().collect();
        names.extend(self.url.as_deref().and_then(tldr::project_name));
        // `zegrep` is an alias of `zgrep --extended-regexp`: only plain names.
        names.extend(self.alias_of.iter().filter(|a| !a.contains(' ')).cloned());
        // `docker image` -> "docker" says nothing new.
        let first_word = self.name.split(' ').next().unwrap_or_default();
        names.retain(|n| !n.eq_ignore_ascii_case(&self.name) && !n.eq_ignore_ascii_case(first_word));
        names.dedup();
        names
    }

    pub fn summary(&self) -> &str {
        self.tldr.as_deref().or(self.man.as_deref()).unwrap_or("")
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Index {
    version: u32,
    /// Embedding model the vectors were made with, if any.
    model: Option<String>,
    pub tools: Vec<Tool>,
    /// Stored next to the index in `vectors.bin`.
    #[serde(skip)]
    pub vectors: Option<Vectors>,
}

/// Two unit vectors per tool, row-major, in the same order as `tools`.
#[derive(Debug)]
pub struct Vectors {
    pub dim: usize,
    /// Name + descriptions.
    pub desc: Vec<f32>,
    /// tldr example descriptions ("Recursively search for a pattern in files").
    pub examples: Vec<f32>,
}

impl Vectors {
    pub fn desc(&self, i: usize) -> &[f32] {
        &self.desc[i * self.dim..(i + 1) * self.dim]
    }
    pub fn examples(&self, i: usize) -> &[f32] {
        &self.examples[i * self.dim..(i + 1) * self.dim]
    }
}

pub fn cache_dir() -> Result<PathBuf> {
    let dir = dirs::cache_dir().context("no cache directory")?.join("wica");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn tldr_zip() -> Result<PathBuf> {
    Ok(cache_dir()?.join("tldr-pages.en.zip"))
}

fn index_file() -> Result<PathBuf> {
    Ok(cache_dir()?.join("index.json"))
}

fn vectors_file() -> Result<PathBuf> {
    Ok(cache_dir()?.join("vectors.bin"))
}

/// Load the index, rebuilding it if missing, outdated, or older than `max_age`.
/// Also rebuilds when an embedding model is available but the index lacks
/// vectors from it (e.g. right after downloading the model).
pub fn load_or_build(max_age: Duration, embedder: Option<&Embedder>) -> Result<Index> {
    let file = index_file()?;
    let fresh = std::fs::metadata(&file)
        .and_then(|m| m.modified())
        .is_ok_and(|t| SystemTime::now().duration_since(t).unwrap_or_default() < max_age);
    if fresh {
        if let Ok(index) = load(&file) {
            let wanted = embedder.map(|_| embed::model_name());
            if index.version == VERSION && index.model == wanted {
                return Ok(index);
            }
        }
    }
    let index = build(embedder)?;
    save(&index)?;
    Ok(index)
}

/// Load an index file. Older versions load too (new fields default), so their
/// vectors can be reused when rebuilding.
fn load(file: &Path) -> Result<Index> {
    let mut index: Index = serde_json::from_slice(&std::fs::read(file)?)?;
    if index.model.is_some() {
        let (dim, data) = embed::load_vectors(&vectors_file()?)?;
        let n = index.tools.len() * dim;
        anyhow::ensure!(data.len() == 2 * n, "vector file does not match index");
        let (desc, examples) = data.split_at(n);
        index.vectors = Some(Vectors { dim, desc: desc.to_vec(), examples: examples.to_vec() });
    }
    Ok(index)
}

pub fn save(index: &Index) -> Result<()> {
    if let Some(v) = &index.vectors {
        embed::save_vectors(&vectors_file()?, v.dim, &[v.desc.as_slice(), &v.examples].concat())?;
    }
    let file = index_file()?;
    let tmp = file.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(index)?)?;
    std::fs::rename(tmp, file)?;
    Ok(())
}

pub fn build(embedder: Option<&Embedder>) -> Result<Index> {
    let exes = path::executables();
    let man = man::descriptions(&path::path_dirs());
    let pages = match tldr_zip() {
        Ok(zip) if zip.exists() => tldr::load(&zip)?,
        _ => Vec::new(),
    };

    let documented: HashSet<String> = man.keys().cloned().collect();
    let mut tools: HashMap<String, Tool> = HashMap::new();
    let new_tool = |key: &str, name: String| Tool {
        installed: is_installed(&exes, &documented, key, &name),
        name,
        tldr: None,
        man: None,
        examples: Vec::new(),
        package: None,
        url: None,
        alias_of: None,
        see_also: Vec::new(),
    };

    for page in pages {
        let tool = tools
            .entry(page.key.clone())
            .or_insert_with(|| new_tool(&page.key, page.title.clone()));
        tool.tldr = Some(page.description);
        tool.examples = page.examples;
        tool.url = page.url;
        tool.alias_of = page.alias_of;
        tool.see_also = page.see_also;
    }
    for (key, desc) in man {
        let name = subcommand_name(&exes, &key).unwrap_or_else(|| key.clone());
        let tool = tools.entry(key.clone()).or_insert_with(|| new_tool(&key, name));
        tool.man = Some(desc);
    }
    // Installed tools without any docs are still findable by name.
    for key in exes.keys() {
        tools.entry(key.clone()).or_insert_with(|| new_tool(key, key.clone()));
    }

    for (key, pkg) in owners::packages(&exes) {
        if let Some(tool) = tools.get_mut(&key) {
            tool.package = Some(pkg);
        }
    }

    let mut tools: Vec<Tool> = tools.into_values().collect();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    let previous = embedder
        .and_then(|_| load(&index_file().ok()?).ok())
        .filter(|p| p.model.as_deref() == Some(embed::model_name().as_str()));
    let vectors = embedder.map(|e| embed_tools(e, &tools, previous.as_ref()));
    Ok(Index { version: VERSION, model: vectors.as_ref().map(|_| embed::model_name()), tools, vectors })
}

fn embed_tools(embedder: &Embedder, tools: &[Tool], previous: Option<&Index>) -> Vectors {
    let desc: Vec<String> = tools.iter().map(embedding_text).collect();
    let examples: Vec<String> = tools.iter().map(examples_text).collect();

    // Reuse vectors for texts that haven't changed since the last build, so
    // daily rebuilds only embed new tools (matters for transformer models).
    let prev_texts: Vec<(String, String)> = previous
        .map(|p| p.tools.iter().map(|t| (embedding_text(t), examples_text(t))).collect())
        .unwrap_or_default();
    let mut cache: HashMap<&str, &[f32]> = HashMap::new();
    if let Some(v) = previous.and_then(|p| p.vectors.as_ref()) {
        for (i, (d, e)) in prev_texts.iter().enumerate() {
            cache.insert(d, v.desc(i));
            cache.insert(e, v.examples(i));
        }
    }
    let dim = embedder.dim();
    let embed = |texts: &[String]| -> Vec<f32> {
        let missing: Vec<String> = texts.iter().filter(|t| !cache.contains_key(t.as_str())).cloned().collect();
        if missing.len() > 500 {
            eprintln!("Embedding {} texts (only needed once, may take a minute)...", missing.len());
        }
        let fresh = embedder.embed_many(&missing);
        let fresh: HashMap<&str, &[f32]> = missing.iter().map(String::as_str).zip(fresh.chunks(dim)).collect();
        texts.iter().flat_map(|t| cache.get(t.as_str()).or(fresh.get(t.as_str())).unwrap().to_vec()).collect()
    };
    Vectors { dim, desc: embed(&desc), examples: embed(&examples) }
}

fn examples_text(t: &Tool) -> String {
    t.examples.iter().map(|e| e.description.as_str()).collect::<Vec<_>>().join(". ")
}

/// The text whose meaning represents a tool. Static embeddings average word
/// vectors, so every extra word dilutes the meaning: keep it to the core
/// description and drop boilerplate ("See also: ...", "More information").
fn embedding_text(t: &Tool) -> String {
    let Some(desc) = t.tldr.as_deref().or(t.man.as_deref()) else {
        return t.name.replace(['-', '_'], " ");
    };
    desc.split_inclusive(". ")
        .filter(|s| !s.starts_with("See also") && !s.starts_with("More information"))
        .collect()
}

/// `rg` is installed if `rg` is on PATH. A subcommand like `git commit` is
/// installed if `git` is on PATH *and* it has a man page (`git-commit.1`);
/// otherwise it may come from an add-on that isn't installed (e.g. git-extras).
fn is_installed(exes: &HashMap<String, PathBuf>, documented: &HashSet<String>, key: &str, name: &str) -> bool {
    exes.contains_key(key)
        || (documented.contains(key) && name.split_once(' ').is_some_and(|(cmd, _)| exes.contains_key(cmd)))
}

/// Man pages like `git-commit` or `ip-link` document subcommands: show them as
/// `git commit` when `git-commit` itself isn't a binary but `git` is.
fn subcommand_name(exes: &HashMap<String, PathBuf>, key: &str) -> Option<String> {
    if exes.contains_key(key) {
        return None;
    }
    let (cmd, rest) = key.split_once('-')?;
    exes.contains_key(cmd).then(|| format!("{cmd} {}", rest.replace('-', " ")))
}
