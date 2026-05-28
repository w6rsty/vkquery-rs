//! BM25Okapi lexical search over prose sections + VUID text.
//!
//! Re-implementation of `rank_bm25.BM25Okapi` (Python). Same defaults:
//! k1=1.5, b=0.75, epsilon=0.25. IDF computed as
//! `log(N - n + 0.5) - log(n + 0.5)`; negative IDFs are clamped to
//! `epsilon * mean(idf)` so highly-common terms don't subtract from scores.
//!
//! Persisted as plain JSON in `<shard>/bm25/`:
//! - `docs.json`: array of indexed docs (tokens + metadata).
//! - `meta.json`: corpus stats.
//!
//! At query time we load `docs.json`, recompute IDFs (~22K docs, <1s), and
//! score in pure Rust.

use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::types::{SearchHit, SearchHitKind};

pub const K1: f64 = 1.5;
pub const B: f64 = 0.75;
pub const EPSILON: f64 = 0.25;

static TOKEN_RE: OnceLock<Regex> = OnceLock::new();

fn token_re() -> &'static Regex {
    TOKEN_RE.get_or_init(|| Regex::new(r"[A-Za-z][A-Za-z0-9_]*").unwrap())
}

/// Split CamelCase / acronym runs into their pieces, matching Python's
/// `re.findall(r"[A-Z][a-z0-9]+|[A-Z]+(?=[A-Z]|$)|[a-z0-9]+", tok)`.
fn camel_split(tok: &str) -> Vec<String> {
    let chars: Vec<char> = tok.chars().collect();
    let n = chars.len();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c.is_ascii_uppercase() {
            let mut j = i;
            while j < n && chars[j].is_ascii_uppercase() {
                j += 1;
            }
            if j < n && chars[j].is_ascii_lowercase() && j - i > 1 {
                out.push(chars[i..j - 1].iter().collect());
                i = j - 1;
            } else if j < n && chars[j].is_ascii_lowercase() {
                let mut k = j;
                while k < n && (chars[k].is_ascii_lowercase() || chars[k].is_ascii_digit()) {
                    k += 1;
                }
                out.push(chars[i..k].iter().collect());
                i = k;
            } else {
                out.push(chars[i..j].iter().collect());
                i = j;
            }
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            let mut j = i;
            while j < n && (chars[j].is_ascii_lowercase() || chars[j].is_ascii_digit()) {
                j += 1;
            }
            out.push(chars[i..j].iter().collect());
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Tokenize a chunk of text. Each identifier-like word emits three variants:
/// original, lowercase, and CamelCase-split lowercased pieces. Matches
/// Python's `tokenize` in `vkquery.search.bm25_index`.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in token_re().find_iter(text) {
        let tok = m.as_str();
        out.push(tok.to_string());
        let lower = tok.to_ascii_lowercase();
        out.push(lower.clone());
        for sub in camel_split(tok) {
            let s = sub.to_ascii_lowercase();
            if s != lower {
                out.push(s);
            }
        }
    }
    out
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Bm25Doc {
    pub kind: String, // "section" | "vuid"
    pub source_id: String,
    pub source_file: Option<String>,
    pub section_anchor: Option<String>,
    pub entity_hint: Option<String>,
    pub text: String,
    pub tokens: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Bm25Meta {
    pub k1: f64,
    pub b: f64,
    pub epsilon: f64,
    pub corpus_size: usize,
    pub avgdl: f64,
}

#[derive(Debug)]
pub struct Bm25 {
    pub docs: Vec<Bm25Doc>,
    pub meta: Bm25Meta,
    pub doc_freqs: Vec<HashMap<String, u32>>,
    pub doc_lens: Vec<u32>,
    pub idf: HashMap<String, f64>,
}

impl Bm25 {
    pub fn from_docs(docs: Vec<Bm25Doc>) -> Self {
        let n = docs.len();
        let mut doc_freqs: Vec<HashMap<String, u32>> = Vec::with_capacity(n);
        let mut doc_lens: Vec<u32> = Vec::with_capacity(n);
        let mut df: HashMap<String, u32> = HashMap::new();
        let mut total_len: u64 = 0;
        for d in &docs {
            let mut tf: HashMap<String, u32> = HashMap::new();
            for t in &d.tokens {
                *tf.entry(t.clone()).or_insert(0) += 1;
            }
            for term in tf.keys() {
                *df.entry(term.clone()).or_insert(0) += 1;
            }
            doc_lens.push(d.tokens.len() as u32);
            total_len += d.tokens.len() as u64;
            doc_freqs.push(tf);
        }
        let avgdl = if n == 0 { 0.0 } else { total_len as f64 / n as f64 };

        let mut raw: HashMap<String, f64> = HashMap::with_capacity(df.len());
        let mut idf_sum: f64 = 0.0;
        let mut neg_terms: Vec<String> = Vec::new();
        for (term, freq) in &df {
            let f = *freq as f64;
            let val = ((n as f64) - f + 0.5).ln() - (f + 0.5).ln();
            raw.insert(term.clone(), val);
            idf_sum += val;
            if val < 0.0 {
                neg_terms.push(term.clone());
            }
        }
        let avg_idf = if raw.is_empty() { 0.0 } else { idf_sum / raw.len() as f64 };
        let eps = EPSILON * avg_idf;
        for t in &neg_terms {
            raw.insert(t.clone(), eps);
        }
        let meta = Bm25Meta { k1: K1, b: B, epsilon: EPSILON, corpus_size: n, avgdl };
        Bm25 { docs, meta, doc_freqs, doc_lens, idf: raw }
    }

    pub fn search(&self, query: &str, k: usize) -> Vec<SearchHit> {
        let q_tokens = tokenize(query);
        let n = self.docs.len();
        let mut scores: Vec<f64> = vec![0.0; n];
        for q in &q_tokens {
            let Some(idf) = self.idf.get(q) else { continue };
            for (i, tf) in self.doc_freqs.iter().enumerate() {
                let f = tf.get(q).copied().unwrap_or(0) as f64;
                if f == 0.0 {
                    continue;
                }
                let dl = self.doc_lens[i] as f64;
                let denom = f + K1 * (1.0 - B + B * dl / self.meta.avgdl);
                scores[i] += idf * (f * (K1 + 1.0)) / denom;
            }
        }
        let mut ranked: Vec<(f64, usize)> = scores
            .iter()
            .enumerate()
            .filter(|(_, s)| **s > 0.0)
            .map(|(i, s)| (*s, i))
            .collect();
        ranked.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1.cmp(&b.1))
        });
        ranked
            .into_iter()
            .take(k)
            .map(|(score, i)| {
                let d = &self.docs[i];
                let snippet: String = d.text.chars().take(240).collect();
                SearchHit {
                    score: score as f32,
                    kind: if d.kind == "vuid" {
                        SearchHitKind::Vuid
                    } else {
                        SearchHitKind::Section
                    },
                    source_file: d.source_file.clone(),
                    section_anchor: d.section_anchor.clone(),
                    entity_hint: d.entity_hint.clone(),
                    snippet,
                }
            })
            .collect()
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir).with_context(|| format!("mkdir {}", dir.display()))?;
        let docs_path = dir.join("docs.json");
        let meta_path = dir.join("meta.json");
        let docs_blob = serde_json::to_vec(&self.docs)?;
        std::fs::write(&docs_path, docs_blob)
            .with_context(|| format!("write {}", docs_path.display()))?;
        let meta_blob = serde_json::to_vec_pretty(&self.meta)?;
        std::fs::write(&meta_path, meta_blob)
            .with_context(|| format!("write {}", meta_path.display()))?;
        Ok(())
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let docs_path = dir.join("docs.json");
        let bytes = std::fs::read(&docs_path)
            .with_context(|| format!("read {}", docs_path.display()))?;
        let docs: Vec<Bm25Doc> = serde_json::from_slice(&bytes)?;
        Ok(Self::from_docs(docs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_split_basic() {
        assert_eq!(
            camel_split("VkImageCreateInfo"),
            vec!["Vk", "Image", "Create", "Info"]
        );
        assert_eq!(camel_split("RGBA"), vec!["RGBA"]);
        assert_eq!(camel_split("RGBAValue"), vec!["RGBA", "Value"]);
        assert_eq!(camel_split("vk_filter_cubic"), vec!["vk", "filter", "cubic"]);
    }

    #[test]
    fn tokenize_emits_three_variants() {
        let toks = tokenize("VkImageCreateInfo");
        assert!(toks.contains(&"VkImageCreateInfo".into()));
        assert!(toks.contains(&"vkimagecreateinfo".into()));
        assert!(toks.contains(&"image".into()));
        assert!(toks.contains(&"create".into()));
        assert!(toks.contains(&"info".into()));
    }

    #[test]
    fn bm25_search_finds_obvious_match() {
        // Need ≥3 docs so a single-term IDF isn't 0 — with N=2 and a term in
        // 1 doc, `log(1.5) - log(1.5) == 0` and the score filter drops it.
        let docs = vec![
            Bm25Doc {
                kind: "section".into(),
                source_id: "a".into(),
                text: "command buffer recording state must be active".into(),
                tokens: tokenize("command buffer recording state must be active"),
                ..Default::default()
            },
            Bm25Doc {
                kind: "section".into(),
                source_id: "b".into(),
                text: "image layout transition requires barriers".into(),
                tokens: tokenize("image layout transition requires barriers"),
                ..Default::default()
            },
            Bm25Doc {
                kind: "section".into(),
                source_id: "c".into(),
                text: "queue submit and fences for synchronization".into(),
                tokens: tokenize("queue submit and fences for synchronization"),
                ..Default::default()
            },
        ];
        let index = Bm25::from_docs(docs);
        let hits = index.search("image layout", 5);
        assert!(!hits.is_empty());
        assert!(hits[0].snippet.contains("image layout"));
    }
}
