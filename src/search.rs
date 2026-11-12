//! Hybrid search: semantic similarity (embeddings) plus BM25 keyword matching
//! with stemming and a small synonym table.

use std::collections::{HashMap, HashSet};

use rayon::prelude::*;
use rust_stemmers::{Algorithm, Stemmer};

use crate::embed::{self, Embedder};
use crate::index::{Index, Tool};
use crate::sources::tldr::Example;

// Field weights: what a match in each field is worth.
const W_NAME: f32 = 3.0;
const W_TAG: f32 = 3.0;
const W_DESC: f32 = 2.0;
const W_EXAMPLE: f32 = 0.4;
/// Synonyms count less than the words the user actually typed.
const W_SYNONYM: f32 = 0.4;

// Hybrid scoring: semantic similarity + ALPHA * normalized keyword score.
// BETA weighs the similarity to the tldr example descriptions.
const ALPHA: f32 = 0.3;
const BETA: f32 = 0.9;
/// Only show results at least this far from the noise floor towards the best match.
const RELATIVE_CUTOFF: f32 = 0.25;
/// The score at this rank is treated as the noise floor.
const NOISE_RANK: usize = 100;
/// Suggest uninstalled tools whose relevance beats the best installed one by this much.
const SUGGEST_MARGIN: f32 = 0.15;
const SUGGESTIONS: usize = 3;

const K1: f32 = 1.2;
const B: f32 = 0.75;

const STOPWORDS: &[&str] = &[
    "a", "an", "and", "the", "of", "in", "on", "for", "to", "with", "from", "by", "or", "into", "at", "as", "is", "it",
    "its", "be", "that", "this", "my", "i", "me", "how", "do", "can", "via", "using", "use",
    // Words that describe every tool and so carry no signal in a query.
    "cli", "tool", "tools", "program", "utility", "app", "command", "commandline", "line",
];

/// Groups of words that mean roughly the same thing.
const SYNONYMS: &[&[&str]] = &[
    &["search", "find", "grep", "match", "lookup"],
    &["show", "display", "view", "print", "render", "preview"],
    &["delete", "remove", "erase", "wipe"],
    &["copy", "duplicate", "clone"],
    &["move", "rename"],
    &["download", "fetch"],
    &["edit", "modify", "change"],
    &["compress", "archive", "pack"],
    &["extract", "unpack", "decompress", "unzip"],
    &["convert", "transform", "transcode"],
    &["list", "enumerate", "ls"],
    &["directory", "folder", "dir"],
    &["disk", "storage", "drive"],
    &["process", "task", "pid"],
    &["monitor", "watch", "top"],
    &["kill", "terminate", "stop"],
    &["image", "picture", "photo"],
    &["video", "movie"],
    &["benchmark", "measure", "time"],
];

pub struct Results<'a> {
    pub shown: Vec<Hit<'a>>,
    /// Not installed, but a clearly better fit than anything installed.
    pub suggestions: Vec<Hit<'a>>,
}

pub struct Hit<'a> {
    pub tool: &'a Tool,
    /// Score parts for `--debug`: (semantic similarity, normalized keyword score).
    pub parts: (f32, f32),
    pub score: f32,
    /// 0 = noise floor, 1 = best match for this query.
    pub relevance: f32,
    pub example: Option<&'a Example>,
}

pub struct Searcher {
    stemmer: Stemmer,
    stop: HashSet<&'static str>,
}

impl Searcher {
    pub fn new() -> Self {
        Self { stemmer: Stemmer::create(Algorithm::English), stop: STOPWORDS.iter().copied().collect() }
    }

    fn tokens(&self, text: &str) -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .filter(|w| !self.stop.contains(w.as_str()))
            .map(|w| self.stemmer.stem(&w).into_owned())
            .collect()
    }

    /// Query terms with weights: typed words at 1.0, synonyms lower.
    fn query_terms(&self, query: &str) -> (Vec<String>, HashMap<String, f32>) {
        let typed = self.tokens(query);
        let mut terms: HashMap<String, f32> = typed.iter().map(|t| (t.clone(), 1.0)).collect();
        for group in SYNONYMS {
            let stems: Vec<String> = group.iter().map(|w| self.stemmer.stem(w).into_owned()).collect();
            if stems.iter().any(|s| typed.contains(s)) {
                for s in stems {
                    terms.entry(s).or_insert(W_SYNONYM);
                }
            }
        }
        (typed, terms)
    }

    fn document(&self, tool: &Tool, tags: Option<&Vec<String>>) -> (HashMap<String, f32>, f32) {
        let mut tf: HashMap<String, f32> = HashMap::new();
        let mut len = 0.0;
        let mut add = |text: &str, w: f32| {
            for t in self.tokens(text) {
                *tf.entry(t).or_default() += w;
                len += w;
            }
        };
        add(&tool.name, W_NAME);
        // Also index the whole name so `docker-compose` matches as one word.
        add(&tool.name.replace(['-', '_', '.'], ""), W_NAME);
        // `wica ripgrep` should find `rg`.
        for other in tool.other_names() {
            add(&other, W_NAME);
        }
        if let Some(tags) = tags {
            add(&tags.join(" "), W_TAG);
        }
        if let Some(d) = &tool.tldr {
            add(d, W_DESC);
        }
        if let Some(d) = &tool.man {
            add(d, W_DESC);
        }
        for ex in &tool.examples {
            add(&ex.description, W_EXAMPLE);
        }
        (tf, len)
    }

    /// Rank all tools for `query`. With `all`, installed and uninstalled tools
    /// are mixed; otherwise installed tools are shown and uninstalled ones that
    /// fit clearly better are returned separately as suggestions.
    pub fn search<'a>(
        &self,
        index: &'a Index,
        tags: &HashMap<String, Vec<String>>,
        query: &str,
        embedder: Option<&Embedder>,
        all: bool,
        limit: usize,
    ) -> Results<'a> {
        let tools = &index.tools;
        let keyword = self.keyword_scores(tools, tags, query);
        let max_kw = keyword.iter().copied().fold(0.0, f32::max).max(f32::EPSILON);

        let semantic = match (embedder, &index.vectors) {
            (Some(e), Some(v)) if v.dim == e.dim() => Some((e.embed_query(query), v)),
            _ => None,
        };
        let (alpha, beta) = (tuning("WICA_ALPHA", ALPHA), tuning("WICA_BETA", BETA));

        let mut ranked: Vec<Hit> = tools
            .iter()
            .enumerate()
            .filter_map(|(i, tool)| {
                let kw = keyword[i] / max_kw;
                let mut sem = 0.0;
                let mut score = match &semantic {
                    Some((q, vectors)) => {
                        let desc = finite(embed::cosine(q, vectors.desc(i)));
                        let ex = if tool.examples.is_empty() {
                            0.0
                        } else {
                            finite(embed::cosine(q, vectors.examples(i)))
                        };
                        sem = desc.max(beta * ex);
                        sem + alpha * kw
                    }
                    None => kw,
                };
                // Typing the exact name (`wica git`) should put that tool first.
                if tool.name.eq_ignore_ascii_case(query.trim()) {
                    score += 1.0;
                }
                (score > 0.0).then_some(Hit { tool, score, relevance: 0.0, parts: (sem, kw), example: None })
            })
            .collect();
        ranked.sort_by(|a, b| b.score.total_cmp(&a.score));

        // Scores are only meaningful relative to this query's best match and to
        // its noise floor (what a surely-irrelevant tool scores). If glow scores
        // 1.16 for "render markdown" and random tools ~0.6, blender at 0.7 is noise.
        let best = ranked.first().map_or(0.0, |h| h.score);
        let floor = ranked.get(NOISE_RANK).map_or(0.0, |h| h.score).min(best);
        let cutoff = floor + (best - floor) * tuning("WICA_RELATIVE", RELATIVE_CUTOFF);
        for h in &mut ranked {
            h.relevance = (h.score - floor) / (best - floor).max(f32::EPSILON);
        }
        let relevant = ranked.into_iter().take_while(|h| h.score >= cutoff);

        let mut results = Results { shown: Vec::new(), suggestions: Vec::new() };
        if all {
            results.shown = relevant.take(limit).collect();
        } else {
            let (installed, missing): (Vec<_>, Vec<_>) = relevant.partition(|h| h.tool.installed);
            let best_installed = installed.first().map_or(0.0, |h| h.relevance);
            results.suggestions = missing
                .into_iter()
                .take_while(|h| h.relevance > best_installed + SUGGEST_MARGIN)
                .take(SUGGESTIONS)
                .collect();
            results.shown = installed.into_iter().take(limit).collect();
        }

        let query_vec = semantic.as_ref().map(|(q, _)| q.as_slice());
        let terms = self.query_terms(query).1;
        for hit in results.shown.iter_mut().chain(&mut results.suggestions) {
            hit.example = match (query_vec, embedder) {
                (Some(q), Some(e)) => best_example_semantic(hit.tool, q, e),
                _ => self.best_example(hit.tool, &terms),
            };
        }
        results
    }

    /// BM25 score per tool (0 for no match), in the same order as `tools`.
    fn keyword_scores(&self, tools: &[Tool], tags: &HashMap<String, Vec<String>>, query: &str) -> Vec<f32> {
        let (typed, terms) = self.query_terms(query);
        if terms.is_empty() {
            return vec![0.0; tools.len()];
        }
        let docs: Vec<_> = tools
            .par_iter()
            .map(|t| self.document(t, tags.get(t.name.as_str())))
            .collect();

        let n = docs.len() as f32;
        let avg_len = docs.iter().map(|(_, l)| l).sum::<f32>() / n.max(1.0);
        let idf: HashMap<&str, f32> = terms
            .keys()
            .map(|t| {
                let df = docs.iter().filter(|(tf, _)| tf.contains_key(t)).count() as f32;
                (t.as_str(), (1.0 + (n - df + 0.5) / (df + 0.5)).ln())
            })
            .collect();

        docs.iter()
            .map(|(tf, len)| {
                // Synonyms alone are too weak a signal.
                let covered = typed.iter().filter(|t| tf.contains_key(*t)).count();
                if covered == 0 {
                    return 0.0;
                }
                let mut score = 0.0;
                for (term, qw) in &terms {
                    let Some(&f) = tf.get(term) else { continue };
                    let norm = f * (K1 + 1.0) / (f + K1 * (1.0 - B + B * len / avg_len));
                    score += qw * idf[term.as_str()] * norm;
                }
                // Prefer tools that match *all* the typed words.
                score * (0.4 + 0.6 * covered as f32 / typed.len().max(1) as f32)
            })
            .collect()
    }

    /// The tldr example whose description best matches the query.
    fn best_example<'a>(&self, tool: &'a Tool, terms: &HashMap<String, f32>) -> Option<&'a Example> {
        tool.examples
            .iter()
            .map(|ex| {
                let score: f32 = self
                    .tokens(&ex.description)
                    .iter()
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .filter_map(|t| terms.get(t))
                    .sum();
                (ex, score)
            })
            .filter(|(_, s)| *s > 0.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(ex, _)| ex)
    }
}

/// The tldr example closest in meaning to the query.
fn best_example_semantic<'a>(tool: &'a Tool, query: &[f32], embedder: &Embedder) -> Option<&'a Example> {
    let texts: Vec<String> = tool.examples.iter().map(|e| e.description.clone()).collect();
    if texts.is_empty() {
        return None;
    }
    let vectors = embedder.embed_many(&texts);
    let dim = embedder.dim();
    tool.examples
        .iter()
        .enumerate()
        .map(|(i, ex)| (ex, finite(embed::cosine(query, &vectors[i * dim..(i + 1) * dim]))))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(ex, _)| ex)
}

fn finite(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// Read a tuning constant from the environment (for experimenting), else the default.
fn tuning(var: &str, default: f32) -> f32 {
    std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}
