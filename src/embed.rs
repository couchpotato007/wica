//! Semantic search with a small static embedding model (Model2Vec, pure Rust).
//!
//! The model maps text to a vector; texts with similar meaning get similar
//! vectors, so "render markdown" lands near "Display Markdown in the terminal"
//! even without shared keywords, and far from "3D modeling and rendering".

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use model2vec_rs::model::StaticModel;

mod bert;

/// Override with `WICA_MODEL=<huggingface repo>`, e.g. `minishlab/potion-base-8M`
/// for a static model that is 4x smaller and indexes in a fraction of a second.
pub const DEFAULT_MODEL: &str = "BAAI/bge-small-en-v1.5";
const FILES: &[&str] = &["config.json", "tokenizer.json", "model.safetensors"];

pub fn model_name() -> String {
    std::env::var("WICA_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string())
}

pub fn model_dir(cache: &Path) -> PathBuf {
    cache.join("models").join(model_name().replace('/', "--"))
}

pub fn download(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let repo = model_name();
    for file in FILES {
        let url = format!("https://huggingface.co/{repo}/resolve/main/{file}");
        let bytes = ureq::get(&url)
            .call()
            .with_context(|| format!("downloading {url}"))?
            .body_mut()
            .with_config()
            .limit(512 * 1024 * 1024)
            .read_to_vec()?;
        let tmp = dir.join(format!("{file}.part"));
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, dir.join(file))?;
    }
    Ok(())
}

pub enum Embedder {
    /// Model2Vec: a lookup table of word vectors, averaged. Tiny and instant,
    /// but blind to word order and context.
    Static(StaticModel),
    /// A small BERT-style transformer (e.g. bge-small). Understands that
    /// "video editor" means "edit video", at the cost of slower indexing.
    Bert(Box<bert::BertEmbedder>),
}

impl Embedder {
    /// Load the model if it has been downloaded (`wica update`).
    pub fn load(dir: &Path) -> Result<Option<Self>> {
        if !FILES.iter().all(|f| dir.join(f).exists()) {
            return Ok(None);
        }
        let read = |f: &str| std::fs::read(dir.join(f)).with_context(|| format!("reading model file {f}"));
        let config = read("config.json")?;
        let is_bert = serde_json::from_slice::<serde_json::Value>(&config)?
            .get("model_type")
            .and_then(|v| v.as_str())
            .is_some_and(|t| t == "bert");
        if is_bert {
            return Ok(Some(Self::Bert(Box::new(bert::BertEmbedder::load(dir, &model_name())?))));
        }
        let model = StaticModel::from_bytes(read("tokenizer.json")?, read("model.safetensors")?, config, Some(true))?;
        Ok(Some(Self::Static(model)))
    }

    pub fn dim(&self) -> usize {
        match self {
            Self::Static(m) => m.dim(),
            Self::Bert(m) => m.dim(),
        }
    }

    /// Row-major `texts.len() * dim` buffer of unit vectors.
    pub fn embed_many(&self, texts: &[String]) -> Vec<f32> {
        match self {
            Self::Static(m) => m.encode_flat(texts),
            Self::Bert(m) => m.embed(texts, false).expect("embedding failed"),
        }
    }

    /// Embed a search query. Some retrieval models expect a query prefix.
    pub fn embed_query(&self, text: &str) -> Vec<f32> {
        match self {
            Self::Static(m) => m.encode_single(text),
            Self::Bert(m) => m.embed(&[text.to_string()], true).expect("embedding failed"),
        }
    }
}

/// Vectors are normalized, so the dot product is the cosine similarity.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Store vectors as raw little-endian f32: `[dim: u32][f32 * n * dim]`.
pub fn save_vectors(path: &Path, dim: usize, data: &[f32]) -> Result<()> {
    let mut bytes = Vec::with_capacity(4 + data.len() * 4);
    bytes.extend_from_slice(&(dim as u32).to_le_bytes());
    for v in data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

pub fn load_vectors(path: &Path) -> Result<(usize, Vec<f32>)> {
    let bytes = std::fs::read(path)?;
    anyhow::ensure!(bytes.len() >= 4 && (bytes.len() - 4) % 4 == 0, "corrupt vector file");
    let dim = u32::from_le_bytes(bytes[..4].try_into().unwrap()) as usize;
    let data = bytes[4..]
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect();
    Ok((dim, data))
}
