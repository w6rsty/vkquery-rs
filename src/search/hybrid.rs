//! Reciprocal Rank Fusion between BM25 (lexical) and embeddings (semantic).
//!
//! Mirrors Python `vkquery.api._fuse_rrf`. Constant c=60. Dedup key is
//! `(kind, source_file, section_anchor or entity_hint, snippet[..60])`.

use std::collections::HashMap;

use crate::types::SearchHit;

/// RRF damping constant. Defaults to 60.0 (the value from the original RRF
/// paper, matching Python `vkquery.api._fuse_rrf`). Override via
/// `VKQUERY_HYBRID_RRF_K=<positive f32>` to experiment with rank fusion
/// weighting; smaller values push top-of-list items further apart.
fn rrf_constant() -> f32 {
    std::env::var("VKQUERY_HYBRID_RRF_K")
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
        .filter(|v| *v > 0.0)
        .unwrap_or(60.0)
}

type DedupKey = (String, String, String, String);

fn key(hit: &SearchHit) -> DedupKey {
    let anchor = hit
        .section_anchor
        .clone()
        .or_else(|| hit.entity_hint.clone())
        .unwrap_or_default();
    let snippet_prefix: String = hit.snippet.chars().take(60).collect();
    let kind_str = match hit.kind {
        crate::types::SearchHitKind::Section => "section",
        crate::types::SearchHitKind::Vuid => "vuid",
    };
    (
        kind_str.to_string(),
        hit.source_file.clone().unwrap_or_default(),
        anchor,
        snippet_prefix,
    )
}

/// Combine two ranked SearchHit lists via RRF. Returns top-`k` deduplicated
/// hits, with the new `score` set to the fused RRF value and the
/// representative hit picked from whichever list saw it first.
pub fn rrf(bm25: &[SearchHit], embed: &[SearchHit], k: usize) -> Vec<SearchHit> {
    let c = rrf_constant();
    let mut scores: HashMap<DedupKey, f32> = HashMap::new();
    let mut rep: HashMap<DedupKey, SearchHit> = HashMap::new();

    for (rank, hit) in bm25.iter().enumerate() {
        let k_ = key(hit);
        let s = 1.0 / (c + rank as f32 + 1.0);
        *scores.entry(k_.clone()).or_insert(0.0) += s;
        rep.entry(k_).or_insert_with(|| hit.clone());
    }
    for (rank, hit) in embed.iter().enumerate() {
        let k_ = key(hit);
        let s = 1.0 / (c + rank as f32 + 1.0);
        *scores.entry(k_.clone()).or_insert(0.0) += s;
        rep.entry(k_).or_insert_with(|| hit.clone());
    }

    let mut fused: Vec<(f32, SearchHit)> = scores
        .into_iter()
        .map(|(k_, s)| {
            let mut hit = rep.remove(&k_).unwrap();
            hit.score = s;
            (s, hit)
        })
        .collect();
    fused.sort_by(|a, b| {
        b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
    });
    fused.into_iter().take(k).map(|(_, h)| h).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SearchHit, SearchHitKind};

    fn h(kind: SearchHitKind, snip: &str, anchor: Option<&str>) -> SearchHit {
        SearchHit {
            score: 1.0,
            kind,
            source_file: Some("chapters/x.adoc".into()),
            section_anchor: anchor.map(String::from),
            entity_hint: None,
            snippet: snip.into(),
        }
    }

    #[test]
    fn rrf_dedups_across_lists() {
        let a = vec![
            h(SearchHitKind::Section, "first match", Some("a")),
            h(SearchHitKind::Vuid, "lexical only", None),
        ];
        let b = vec![
            h(SearchHitKind::Section, "first match", Some("a")),
            h(SearchHitKind::Vuid, "semantic only", None),
        ];
        let out = rrf(&a, &b, 5);
        // 3 unique items.
        assert_eq!(out.len(), 3);
        // Item that appears in BOTH should score highest.
        assert!(out[0].snippet.contains("first match"));
    }
}
