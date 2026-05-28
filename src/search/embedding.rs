//! candle-backed BERT encoder (default: BAAI/bge-small-en-v1.5) with
//! brute-force ndarray dot-product search.
//!
//! Mirrors Python `vkquery.search.embedding_index`. CLS-pooled, L2-normalized.
//! Stored on disk per shard:
//!   <shard>/embeddings/
//!     vectors.f32        # row-major little-endian f32, shape [N, dim]
//!     meta.jsonl         # one EmbedDoc per row
//!     model.txt          # model id string

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use serde::{Deserialize, Serialize};
use tokenizers::Tokenizer;

use crate::types::{SearchHit, SearchHitKind};

pub const DEFAULT_MODEL: &str = "BAAI/bge-small-en-v1.5";
const MAX_SEQ_LEN: usize = 512;
const BATCH_SIZE: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedDoc {
    pub kind: String, // "section" | "vuid"
    pub source_id: String,
    pub source_file: Option<String>,
    pub section_anchor: Option<String>,
    pub entity_hint: Option<String>,
    pub snippet: String,
}

/// Resolve the model id from env, falling back to `DEFAULT_MODEL`.
pub fn resolved_model() -> String {
    std::env::var("VKQUERY_EMBEDDING_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string())
}

/// One-shot encoder for callers that don't want a persistent index — useful
/// for parity tests and ad-hoc tooling. Re-loads the model on every call;
/// don't use in hot paths.
pub fn encode_texts(model_id: Option<&str>, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    let id = model_id.map(String::from).unwrap_or_else(resolved_model);
    let loaded = load_model(&id)?;
    encode_batch(&loaded, texts)
}

struct LoadedModel {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
    hidden_size: usize,
}

fn model_cache_dir(model_id: &str) -> Result<PathBuf> {
    let base = dirs::cache_dir().ok_or_else(|| anyhow!("no cache dir available"))?;
    let safe_id = model_id.replace('/', "--");
    let dir = base.join("vkquery").join("models").join(safe_id);
    std::fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
    Ok(dir)
}

fn download_file(url: &str, dest: &Path) -> Result<()> {
    if dest.is_file() && dest.metadata()?.len() > 0 {
        return Ok(());
    }
    tracing::info!("downloading {url}");
    let resp = ureq::get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    if resp.status() != 200 {
        return Err(anyhow!("GET {url} returned status {}", resp.status()));
    }
    let mut reader = resp.into_reader();
    let tmp = dest.with_extension("part");
    {
        let mut out = std::fs::File::create(&tmp)
            .with_context(|| format!("create {}", tmp.display()))?;
        std::io::copy(&mut reader, &mut out).with_context(|| format!("write {}", tmp.display()))?;
    }
    std::fs::rename(&tmp, dest)
        .with_context(|| format!("rename {} -> {}", tmp.display(), dest.display()))?;
    Ok(())
}

fn download_model_files(model_id: &str) -> Result<(PathBuf, PathBuf, PathBuf)> {
    let dir = model_cache_dir(model_id)?;
    let base = format!("https://huggingface.co/{model_id}/resolve/main");
    let config = dir.join("config.json");
    let tokenizer = dir.join("tokenizer.json");
    let weights = dir.join("model.safetensors");
    download_file(&format!("{base}/config.json"), &config)?;
    download_file(&format!("{base}/tokenizer.json"), &tokenizer)?;
    download_file(&format!("{base}/model.safetensors"), &weights)?;
    Ok((config, tokenizer, weights))
}

fn load_model(model_id: &str) -> Result<LoadedModel> {
    let (config_path, tokenizer_path, weights_path) = download_model_files(model_id)?;
    let config_bytes =
        std::fs::read(&config_path).with_context(|| format!("read {}", config_path.display()))?;
    let config: Config = serde_json::from_slice(&config_bytes).context("parse config.json")?;
    let hidden_size = config.hidden_size;

    let tokenizer = Tokenizer::from_file(&tokenizer_path)
        .map_err(|e| anyhow!("load tokenizer: {e}"))?;

    let device = Device::Cpu;
    let vb = unsafe {
        VarBuilder::from_mmaped_safetensors(&[weights_path], DType::F32, &device)
            .context("load safetensors via VarBuilder")?
    };
    let model = BertModel::load(vb, &config).context("BertModel::load")?;
    Ok(LoadedModel { model, tokenizer, device, hidden_size })
}

/// Tokenize → forward → CLS pool → L2-normalize a batch of texts.
fn encode_batch(loaded: &LoadedModel, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    // Tokenize with padding to longest-in-batch, truncated to MAX_SEQ_LEN.
    let mut tokenizer = loaded.tokenizer.clone();
    tokenizer
        .with_padding(Some(tokenizers::PaddingParams {
            strategy: tokenizers::PaddingStrategy::BatchLongest,
            direction: tokenizers::PaddingDirection::Right,
            pad_to_multiple_of: None,
            pad_id: 0,
            pad_type_id: 0,
            pad_token: "[PAD]".into(),
        }))
        .with_truncation(Some(tokenizers::TruncationParams {
            max_length: MAX_SEQ_LEN,
            strategy: tokenizers::TruncationStrategy::LongestFirst,
            stride: 0,
            direction: tokenizers::TruncationDirection::Right,
        }))
        .map_err(|e| anyhow!("truncation config: {e}"))?;

    let encodings = tokenizer
        .encode_batch(texts.to_vec(), true)
        .map_err(|e| anyhow!("encode_batch: {e}"))?;
    if encodings.is_empty() {
        return Ok(Vec::new());
    }
    let seq_len = encodings[0].get_ids().len();
    let batch = encodings.len();

    let mut ids: Vec<u32> = Vec::with_capacity(batch * seq_len);
    let mut mask: Vec<u32> = Vec::with_capacity(batch * seq_len);
    let mut types: Vec<u32> = Vec::with_capacity(batch * seq_len);
    for enc in &encodings {
        ids.extend_from_slice(enc.get_ids());
        mask.extend_from_slice(enc.get_attention_mask());
        types.extend_from_slice(enc.get_type_ids());
    }

    let dev = &loaded.device;
    let input_ids = Tensor::from_vec(ids, (batch, seq_len), dev)?;
    let attn = Tensor::from_vec(mask, (batch, seq_len), dev)?;
    let token_type = Tensor::from_vec(types, (batch, seq_len), dev)?;

    // BertModel forward returns sequence_output [batch, seq_len, hidden]
    let out = loaded.model.forward(&input_ids, &token_type, Some(&attn))?;
    // CLS pool: index 0 along seq dim → [batch, hidden]
    let cls = out.i((.., 0, ..))?;
    // L2 normalize along last dim.
    let normed = normalize_l2(&cls)?;
    let host: Vec<Vec<f32>> = normed.to_vec2()?;
    Ok(host)
}

fn normalize_l2(t: &Tensor) -> Result<Tensor> {
    // t: [batch, hidden]
    let norm = t.sqr()?.sum_keepdim(candle_core::D::Minus1)?.sqrt()?;
    // Guard against divide-by-zero by clamping the denominator.
    let safe = norm.clamp(1e-12, f64::INFINITY)?;
    Ok(t.broadcast_div(&safe)?)
}

/// Build the embedding index for a shard.
///
/// Returns (n_docs, hidden_dim, model_id) on success.
pub fn build_index(
    sections: &[crate::index::prose::Section],
    vuids: &std::collections::BTreeMap<String, serde_json::Value>,
    out_dir: &Path,
    model_id: Option<&str>,
) -> Result<(usize, usize, String)> {
    let model_id = model_id.map(String::from).unwrap_or_else(resolved_model);
    let loaded = load_model(&model_id)?;
    let dim = loaded.hidden_size;

    let mut docs: Vec<EmbedDoc> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    for s in sections {
        if s.text.is_empty() {
            continue;
        }
        docs.push(EmbedDoc {
            kind: "section".into(),
            source_id: s.section_id.clone(),
            source_file: Some(s.file.clone()),
            section_anchor: Some(s.section_id.clone()),
            entity_hint: s.refpage_entities.first().cloned(),
            snippet: s.text.chars().take(240).collect(),
        });
        texts.push(s.text.clone());
    }
    for (vid, v) in vuids {
        let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("");
        if text.is_empty() {
            continue;
        }
        docs.push(EmbedDoc {
            kind: "vuid".into(),
            source_id: vid.clone(),
            source_file: v.get("source_file").and_then(|s| s.as_str()).map(String::from),
            section_anchor: None,
            entity_hint: v.get("entity").and_then(|e| e.as_str()).map(String::from),
            snippet: text.chars().take(240).collect(),
        });
        texts.push(text.to_string());
    }

    // Optional cap (handy for fast smoke runs while CPU BERT is the
    // bottleneck — the full corpus at HEAD is ~22K texts and CPU embeds
    // are ~32 vec/30s on Windows without MKL).
    if let Ok(s) = std::env::var("VKQUERY_EMBED_LIMIT") {
        if let Ok(n) = s.parse::<usize>() {
            if n < texts.len() {
                tracing::warn!("VKQUERY_EMBED_LIMIT={n}: truncating from {}", texts.len());
                docs.truncate(n);
                texts.truncate(n);
            }
        }
    }

    if texts.is_empty() {
        return Ok((0, dim, model_id));
    }
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("mkdir {}", out_dir.display()))?;
    let vectors_path = out_dir.join("vectors.f32");
    let meta_path = out_dir.join("meta.jsonl");
    let model_path = out_dir.join("model.txt");

    let mut vec_file =
        std::fs::File::create(&vectors_path).with_context(|| format!("create {}", vectors_path.display()))?;
    let mut meta_file =
        std::fs::File::create(&meta_path).with_context(|| format!("create {}", meta_path.display()))?;

    let total = texts.len();
    let mut written: usize = 0;
    let mut idx = 0;
    while idx < total {
        let end = (idx + BATCH_SIZE).min(total);
        let chunk: Vec<&str> = texts[idx..end].iter().map(|s| s.as_str()).collect();
        let vectors = encode_batch(&loaded, &chunk)
            .with_context(|| format!("encode batch [{idx}..{end})"))?;
        for v in vectors {
            // Write raw little-endian f32.
            let mut buf = Vec::with_capacity(v.len() * 4);
            for f in &v {
                buf.extend_from_slice(&f.to_le_bytes());
            }
            vec_file.write_all(&buf)?;
        }
        for d in &docs[idx..end] {
            let line = serde_json::to_string(d)?;
            meta_file.write_all(line.as_bytes())?;
            meta_file.write_all(b"\n")?;
        }
        written += end - idx;
        idx = end;
        if total >= 50 {
            tracing::debug!("embedded {written}/{total}");
        }
    }
    vec_file.flush()?;
    meta_file.flush()?;
    std::fs::write(&model_path, model_id.as_bytes())?;
    tracing::info!("embeddings: wrote {written} vectors of dim {dim} via {model_id}");
    Ok((written, dim, model_id))
}

/// Search a single shard's embedding index. Re-loads the model on every call
/// (fine for CLI; library users with hot paths should reuse `load_model`).
pub fn search_shard(emb_dir: &Path, query: &str, k: usize) -> Result<Vec<SearchHit>> {
    let model_path = emb_dir.join("model.txt");
    let model_id = std::fs::read_to_string(&model_path)
        .with_context(|| format!("read {}", model_path.display()))?
        .trim()
        .to_string();
    let loaded = load_model(&model_id)?;
    let dim = loaded.hidden_size;

    let meta_path = emb_dir.join("meta.jsonl");
    let meta_bytes = std::fs::read(&meta_path)
        .with_context(|| format!("read {}", meta_path.display()))?;
    let meta_str = std::str::from_utf8(&meta_bytes).context("meta.jsonl not utf-8")?;
    let docs: Vec<EmbedDoc> = meta_str
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).map_err(anyhow::Error::from))
        .collect::<Result<_>>()?;
    let n = docs.len();
    if n == 0 {
        return Ok(Vec::new());
    }

    let vectors_path = emb_dir.join("vectors.f32");
    let vec_bytes = std::fs::read(&vectors_path)
        .with_context(|| format!("read {}", vectors_path.display()))?;
    let expected = n * dim * 4;
    if vec_bytes.len() != expected {
        return Err(anyhow!(
            "vectors.f32 size mismatch: have {} bytes, expected {}",
            vec_bytes.len(),
            expected
        ));
    }
    let mut vectors: Vec<f32> = Vec::with_capacity(n * dim);
    for chunk in vec_bytes.chunks_exact(4) {
        let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        vectors.push(v);
    }

    let qvec = encode_batch(&loaded, &[query])?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("empty query encoding"))?;
    if qvec.len() != dim {
        return Err(anyhow!(
            "query dim {} != index dim {}",
            qvec.len(),
            dim
        ));
    }
    // Brute-force inner product (vectors are L2-normalized so this == cosine).
    let mut scores: Vec<(f32, usize)> = Vec::with_capacity(n);
    for i in 0..n {
        let row = &vectors[i * dim..(i + 1) * dim];
        let mut s: f32 = 0.0;
        for j in 0..dim {
            s += row[j] * qvec[j];
        }
        scores.push((s, i));
    }
    scores.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    Ok(scores
        .into_iter()
        .take(k)
        .map(|(score, i)| {
            let d = &docs[i];
            SearchHit {
                score,
                kind: if d.kind == "vuid" {
                    SearchHitKind::Vuid
                } else {
                    SearchHitKind::Section
                },
                source_file: d.source_file.clone(),
                section_anchor: d.section_anchor.clone(),
                entity_hint: d.entity_hint.clone(),
                snippet: d.snippet.clone(),
            }
        })
        .collect())
}

// `Tensor::i(...)` lives behind the IndexOp trait; bring it in.
use candle_core::IndexOp;
