//! BM25 fixture diff: top-5 results vs the snapshot in
//! `tests/fixtures/bm25_top5_head.json`.
//!
//! Asserts that the two top-5 sets overlap on at least 2 of 5 hits per
//! canonical query, AND that the fixture's top-scoring score lies within
//! 5% of the current top-1 score. The 2/5 threshold tolerates BM25
//! tie-breaking divergence on near-tied documents without losing
//! smoke-test value. Fixture is regenerated with `target/dump_bm25_top5.py`
//! when corpus content changes.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::Deserialize;

use vkquery::cache::Cache;
use vkquery::docs_source::DocsSource;
use vkquery::types::{SearchHit, SearchHitKind};

#[derive(Debug, Deserialize)]
struct FixtureHit {
    kind: String,
    section_anchor: Option<String>,
    entity_hint: Option<String>,
    #[allow(dead_code)]
    source_file: Option<String>,
    score: f64,
}

fn ident(kind: &str, section_anchor: Option<&str>, entity_hint: Option<&str>) -> String {
    match kind {
        "section" => format!("section:{}", section_anchor.unwrap_or("")),
        "vuid" => format!("vuid:{}", entity_hint.unwrap_or("")),
        other => format!("{other}:{}{}", section_anchor.unwrap_or(""), entity_hint.unwrap_or("")),
    }
}

fn rust_ident(h: &SearchHit) -> String {
    let kind = match h.kind {
        SearchHitKind::Section => "section",
        SearchHitKind::Vuid => "vuid",
    };
    ident(kind, h.section_anchor.as_deref(), h.entity_hint.as_deref())
}

fn vulkan_docs_path() -> Option<PathBuf> {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    while let Some(parent) = p.parent() {
        let sibling = parent.join("Vulkan-Docs");
        if sibling.join("xml").join("vk.xml").is_file() {
            return Some(sibling);
        }
        p = parent.to_path_buf();
    }
    None
}

#[test]
fn bm25_top5_overlaps_python_on_canonical_queries() {
    let Some(docs_path) = vulkan_docs_path() else {
        eprintln!("skipping: no sibling Vulkan-Docs clone");
        return;
    };
    let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("bm25_top5_head.json");
    let fixture_text = std::fs::read_to_string(&fixture_path).expect("read bm25 top-5 fixture");
    let fixture: std::collections::BTreeMap<String, Vec<FixtureHit>> =
        serde_json::from_str(&fixture_text).expect("parse bm25 top-5 fixture");

    std::env::set_var("VKQUERY_SKIP_EMBED", "1");
    let dir = tempfile::tempdir().expect("tmpdir");
    let cache = Cache::new(Some(dir.path().to_path_buf()));
    let source = DocsSource::new(Some(docs_path));
    let shard = vkquery::index::build::build_shard(&source, &cache, "HEAD", true)
        .expect("build_shard HEAD for BM25 parity");
    let bm25 = vkquery::search::bm25::Bm25::load(&shard.bm25_dir())
        .expect("load BM25 index from freshly-built shard");

    let mut failures: Vec<String> = Vec::new();
    for (query, py_hits) in &fixture {
        let py_idents: HashSet<String> = py_hits
            .iter()
            .map(|h| ident(&h.kind, h.section_anchor.as_deref(), h.entity_hint.as_deref()))
            .collect();
        let rs_hits = bm25.search(query, 5);
        let rs_idents: HashSet<String> = rs_hits.iter().map(rust_ident).collect();
        let overlap = py_idents.intersection(&rs_idents).count();
        if overlap < 2 {
            failures.push(format!(
                "query {query:?}: overlap {overlap}/5 below 2 (smoke threshold)\n  Python: {py_idents:?}\n  Rust  : {rs_idents:?}"
            ));
            continue;
        }
        // Score parity: top-1 score should be within ~5% of Python's. If it
        // diverges this far, our IDF/tokenizer/avgdl has drifted out of
        // alignment with rank_bm25.BM25Okapi, not a tie issue.
        let py_top = py_hits.first().map(|h| h.score).unwrap_or(0.0);
        let rs_top = rs_hits.first().map(|h| h.score as f64).unwrap_or(0.0);
        let pct = if py_top > 0.0 { ((rs_top - py_top) / py_top).abs() } else { 0.0 };
        if pct > 0.05 {
            failures.push(format!(
                "query {query:?}: top-1 score drift {drift:.2}% (py={py_top:.4}, rs={rs_top:.4})",
                drift = pct * 100.0
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "BM25 parity failures:\n{}",
        failures.join("\n\n")
    );
}
