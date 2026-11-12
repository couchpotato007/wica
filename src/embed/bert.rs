//! Sentence embeddings with a BERT-style model (bge, MiniLM, ...) via candle.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Error as E, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use rayon::prelude::*;
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

/// Descriptions are short; capping the length keeps indexing fast.
const MAX_TOKENS: usize = 64;
const BATCH: usize = 32;

pub struct BertEmbedder {
    model: BertModel,
    tokenizer: Tokenizer,
    dim: usize,
    /// bge models use the [CLS] token and a query instruction; MiniLM-style
    /// models average all tokens.
    cls_pooling: bool,
    query_prefix: &'static str,
}

impl BertEmbedder {
    pub fn load(dir: &Path, name: &str) -> Result<Self> {
        let config: Config = serde_json::from_slice(&std::fs::read(dir.join("config.json"))?)?;
        let dim = serde_json::from_slice::<serde_json::Value>(&std::fs::read(dir.join("config.json"))?)?
            .get("hidden_size")
            .and_then(|v| v.as_u64())
            .unwrap_or(384) as usize;
        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(E::msg)?;
        tokenizer.with_padding(Some(PaddingParams::default()));
        tokenizer
            .with_truncation(Some(TruncationParams { max_length: MAX_TOKENS, ..Default::default() }))
            .map_err(E::msg)?;
        // SAFETY: the file is only read, and we keep no references past the model's lifetime.
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[dir.join("model.safetensors")], DTYPE, &Device::Cpu)? };
        let model = BertModel::load(vb, &config)?;
        let is_bge = name.contains("bge");
        Ok(Self {
            model,
            tokenizer,
            dim,
            cls_pooling: is_bge,
            query_prefix: if is_bge { "Represent this sentence for searching relevant passages: " } else { "" },
        })
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Row-major, L2-normalized embeddings.
    pub fn embed(&self, texts: &[String], query: bool) -> Result<Vec<f32>> {
        // Sort by length so each batch pads as little as possible.
        let mut order: Vec<usize> = (0..texts.len()).collect();
        order.sort_by_key(|&i| texts[i].len());

        let show_progress = texts.len() > BATCH * 4;
        let done = AtomicUsize::new(0);
        // Batches run in parallel: candle alone keeps only a few cores busy.
        let batches: Vec<Vec<f32>> = order
            .par_chunks(BATCH)
            .map(|chunk| {
                let batch: Vec<String> = chunk
                    .iter()
                    .map(|&i| if query { format!("{}{}", self.query_prefix, texts[i]) } else { texts[i].clone() })
                    .collect();
                let v = self.embed_batch(batch);
                let n = done.fetch_add(chunk.len(), Ordering::Relaxed) + chunk.len();
                if show_progress {
                    eprint!("\r  embedding {n}/{} ", texts.len());
                }
                v
            })
            .collect::<Result<_>>()?;
        if show_progress {
            eprintln!();
        }

        let mut out = vec![0.0f32; texts.len() * self.dim];
        for (chunk, vectors) in order.chunks(BATCH).zip(batches) {
            for (row, &i) in vectors.chunks(self.dim).zip(chunk) {
                out[i * self.dim..(i + 1) * self.dim].copy_from_slice(row);
            }
        }
        Ok(out)
    }

    fn embed_batch(&self, texts: Vec<String>) -> Result<Vec<f32>> {
        let enc = self.tokenizer.encode_batch(texts, true).map_err(E::msg)?;
        let ids: Vec<Tensor> = enc.iter().map(|e| Tensor::new(e.get_ids(), &Device::Cpu)).collect::<Result<_, _>>()?;
        let mask: Vec<Tensor> = enc
            .iter()
            .map(|e| Tensor::new(e.get_attention_mask(), &Device::Cpu))
            .collect::<Result<_, _>>()?;
        let ids = Tensor::stack(&ids, 0)?;
        let mask = Tensor::stack(&mask, 0)?;
        let types = ids.zeros_like()?;

        let hidden = self.model.forward(&ids, &types, Some(&mask))?; // [batch, seq, dim]
        let pooled = if self.cls_pooling {
            hidden.narrow(1, 0, 1)?.squeeze(1)?
        } else {
            let m = mask.to_dtype(DType::F32)?.unsqueeze(2)?;
            (hidden.broadcast_mul(&m)?.sum(1)?.broadcast_div(&m.sum(1)?))?
        };
        let norm = pooled.sqr()?.sum_keepdim(1)?.sqrt()?;
        let normalized = pooled.broadcast_div(&norm)?;
        Ok(normalized.flatten_all()?.to_vec1::<f32>()?)
    }
}
