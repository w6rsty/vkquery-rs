//! Snapshot-vs-snapshot comparison. Mirrors `vkquery.index.diff`.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::cache::{Cache, Shard};
use crate::docs_source::DocsSource;
use crate::index::build::build_shard;
use crate::types::{DiffBucket, DiffEntry, DiffReport};

const ALL_KINDS: &[&str] = &[
    "functions",
    "structs",
    "enums",
    "handles",
    "extensions",
    "features",
];

/// Fields whose changes are noise for "did the entity itself change?" —
/// tracked separately (e.g. version availability).
fn noisy_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "functions" | "structs" | "vuids" => &["available_in"],
        "extensions" => &["status"],
        _ => &[],
    }
}

fn project(entity: &Value, noisy: &[&str]) -> Value {
    let Value::Object(map) = entity else { return entity.clone() };
    let filtered: serde_json::Map<String, Value> = map
        .iter()
        .filter(|(k, _)| !noisy.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    Value::Object(filtered)
}

fn struct_hash(entity: &Value, noisy: &[&str]) -> String {
    let projection = project(entity, noisy);
    // serde_json::Map is BTreeMap-backed in default features, so to_string is
    // already key-sorted. That matches Python's `sort_keys=True`.
    let blob = serde_json::to_vec(&projection).unwrap_or_default();
    let mut h = Sha256::new();
    h.update(&blob);
    hex::encode(h.finalize())
}

fn field_diff(a: &Value, b: &Value, noisy: &[&str]) -> serde_json::Map<String, Value> {
    let empty = serde_json::Map::new();
    let am = a.as_object().unwrap_or(&empty);
    let bm = b.as_object().unwrap_or(&empty);
    let mut out: serde_json::Map<String, Value> = serde_json::Map::new();
    let keys: BTreeSet<&String> = am.keys().chain(bm.keys()).collect();
    for k in keys {
        if noisy.contains(&k.as_str()) {
            continue;
        }
        let av = am.get(k).cloned().unwrap_or(Value::Null);
        let bv = bm.get(k).cloned().unwrap_or(Value::Null);
        if av != bv {
            out.insert(k.clone(), json!({"from": av, "to": bv}));
        }
    }
    out
}

fn bucket_diff(a: &Value, b: &Value, kind: &str) -> DiffBucket {
    let empty = serde_json::Map::new();
    let am = a.as_object().unwrap_or(&empty);
    let bm = b.as_object().unwrap_or(&empty);
    let a_keys: BTreeSet<&String> = am.keys().collect();
    let b_keys: BTreeSet<&String> = bm.keys().collect();

    let added: Vec<String> = b_keys.difference(&a_keys).map(|s| (*s).clone()).collect();
    let removed: Vec<String> = a_keys.difference(&b_keys).map(|s| (*s).clone()).collect();
    let noisy = noisy_fields(kind);
    let mut changed: Vec<DiffEntry> = Vec::new();
    for name in a_keys.intersection(&b_keys) {
        let av = &am[name.as_str()];
        let bv = &bm[name.as_str()];
        if struct_hash(av, noisy) != struct_hash(bv, noisy) {
            let fields = field_diff(av, bv, noisy);
            if !fields.is_empty() {
                changed.push(DiffEntry { name: (*name).clone(), fields });
            }
        }
    }
    DiffBucket { added, removed, changed }
}

fn ensure_shard_for(source: &DocsSource, cache: &Cache, tag: &str) -> Result<Shard> {
    let shard = cache.shard_for(source, tag)?;
    if !cache.is_fresh(&shard, crate::index::build::XML_INDEX_NAMES) {
        build_shard(source, cache, tag, false)?;
    }
    cache.shard_for(source, tag)
}

fn load_payloads(source: &DocsSource, cache: &Cache, tag: &str, kinds: &[&str]) -> Result<BTreeMap<String, Value>> {
    let shard = ensure_shard_for(source, cache, tag)?;
    let mut out: BTreeMap<String, Value> = BTreeMap::new();
    for k in kinds {
        let v = shard.read_json::<Value>(k).unwrap_or_else(|_| Value::Object(Default::default()));
        out.insert((*k).to_string(), v);
    }
    Ok(out)
}

pub fn diff_versions(v1: &str, v2: &str, entity: Option<&str>) -> Result<DiffReport> {
    let source = DocsSource::new(None);
    let cache = Cache::new(None);
    let kinds: Vec<&str> = match entity {
        Some(e) if ALL_KINDS.contains(&e) => vec![e],
        Some(other) => anyhow::bail!("unknown entity kind: {other}"),
        None => ALL_KINDS.to_vec(),
    };
    let a = load_payloads(&source, &cache, v1, &kinds)?;
    let b = load_payloads(&source, &cache, v2, &kinds)?;

    let mut report = DiffReport { from_tag: v1.to_string(), to_tag: v2.to_string(), ..Default::default() };
    for k in &kinds {
        let av = a.get(*k).cloned().unwrap_or(Value::Null);
        let bv = b.get(*k).cloned().unwrap_or(Value::Null);
        let bucket = bucket_diff(&av, &bv, k);
        match *k {
            "functions" => report.functions = bucket,
            "structs" => report.structs = bucket,
            "enums" => report.enums = bucket,
            "handles" => report.handles = bucket,
            "extensions" => report.extensions = bucket,
            "features" => report.features = bucket,
            _ => {}
        }
    }

    if kinds.contains(&"extensions") {
        let a_exts = a.get("extensions").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        let b_exts = b.get("extensions").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        let names: BTreeSet<&String> = a_exts.keys().filter(|k| b_exts.contains_key(*k)).collect();
        for name in names {
            let ap = a_exts[name].get("promotedto").cloned().unwrap_or(Value::Null);
            let bp = b_exts[name].get("promotedto").cloned().unwrap_or(Value::Null);
            if ap != bp {
                report.promoted.push(json!({"name": name, "from": ap, "to": bp}));
            }
        }
    }

    Ok(report)
}
