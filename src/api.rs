//! Public query primitives — entry point for library consumers.
//!
//! Each query takes a tag (default `"HEAD"`) so callers can pin against a
//! specific Vulkan-Docs git tag. The function lazily ensures the shard for
//! that tag is fresh, then loads only what it needs.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;

use crate::cache::Cache;
use crate::docs_source::DocsSource;
use crate::index::build::{build_shard, XML_INDEX_NAMES};
use crate::types::*;

pub const DEFAULT_TAG: &str = "HEAD";

fn ensure_shard(tag: &str) -> Result<crate::cache::Shard> {
    let source = DocsSource::new(None);
    let cache = Cache::new(None);
    let shard = cache.shard_for(&source, tag)?;
    if !cache.is_fresh(&shard, XML_INDEX_NAMES) {
        build_shard(&source, &cache, tag, false)?;
    }
    cache.shard_for(&source, tag)
}

// ---- get_function ---------------------------------------------------------

pub fn get_function(name: &str, tag: &str) -> Result<FunctionInfo> {
    let shard = ensure_shard(tag)?;
    let map: serde_json::Map<String, Value> = shard.read_json("functions")?;
    let aliases: std::collections::BTreeMap<String, String> = shard.read_json("aliases")?;
    let (canonical, aliased_from) = resolve_alias(name, &aliases);
    let v = map
        .get(&canonical)
        .ok_or_else(|| anyhow!("function not found: {name}"))?;
    let mut info: FunctionInfo = serde_json::from_value(v.clone())?;
    info.aliased_from = aliased_from;
    info.vuids = lookup_vuids(&shard, &canonical)?;
    Ok(info)
}

fn resolve_alias(
    name: &str,
    aliases: &std::collections::BTreeMap<String, String>,
) -> (String, Option<String>) {
    let mut canonical = name.to_string();
    let mut seen = std::collections::HashSet::new();
    while let Some(next) = aliases.get(&canonical) {
        if !seen.insert(canonical.clone()) {
            break;
        }
        canonical = next.clone();
    }
    let aliased_from = if canonical != name { Some(name.to_string()) } else { None };
    (canonical, aliased_from)
}

fn lookup_vuids(shard: &crate::cache::Shard, entity: &str) -> Result<Vec<Vuid>> {
    let vuids_map: std::collections::BTreeMap<String, Value> =
        shard.read_json("vuids").unwrap_or_default();
    let mut hits: Vec<Vuid> = vuids_map
        .values()
        .filter(|v| v.get("entity").and_then(|e| e.as_str()) == Some(entity))
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect();
    hits.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(hits)
}

// ---- get_struct -----------------------------------------------------------

pub fn get_struct(name: &str, tag: &str) -> Result<StructInfo> {
    let shard = ensure_shard(tag)?;
    let structs: serde_json::Map<String, Value> = shard.read_json("structs")?;
    let aliases: std::collections::BTreeMap<String, String> = shard.read_json("aliases")?;
    let (canonical, aliased_from) = resolve_alias(name, &aliases);
    let mut info: StructInfo = match structs.get(&canonical) {
        Some(v) => serde_json::from_value(v.clone())?,
        None => return Err(anyhow!("struct not found: {name}")),
    };
    info.aliased_from = aliased_from;
    // `extended_by` lives in reverse.json — splice it in.
    let reverse: Value = shard.read_json("reverse")?;
    if let Some(extended) = reverse.get("extended_by").and_then(|v| v.get(&canonical)) {
        info.extended_by = serde_json::from_value(extended.clone()).unwrap_or_default();
    }
    info.vuids = lookup_vuids(&shard, &canonical)?;
    Ok(info)
}

// ---- list_extensions ------------------------------------------------------

pub fn list_extensions(
    tag: &str,
    type_filter: Option<&str>,
    author: Option<&str>,
    status: Option<&str>,
) -> Result<Vec<ExtensionInfo>> {
    let shard = ensure_shard(tag)?;
    let map: serde_json::Map<String, Value> = shard.read_json("extensions")?;
    let mut out: Vec<ExtensionInfo> = Vec::new();
    for (_, v) in map {
        let info: ExtensionInfo = serde_json::from_value(v)?;
        if let Some(t) = type_filter {
            let want = match t {
                "instance" => Some(ExtensionType::Instance),
                "device" => Some(ExtensionType::Device),
                _ => None,
            };
            if info.ty != want {
                continue;
            }
        }
        if let Some(a) = author {
            if info.author.as_deref() != Some(a) {
                continue;
            }
        }
        if let Some(s) = status {
            let want = match s {
                "active" => ExtensionStatus::Active,
                "promoted" => ExtensionStatus::Promoted,
                "deprecated" => ExtensionStatus::Deprecated,
                "obsoleted" => ExtensionStatus::Obsoleted,
                _ => continue,
            };
            if info.status != want {
                continue;
            }
        }
        out.push(info);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

// ---- find_callers ---------------------------------------------------------

pub fn find_callers(type_name: &str, tag: &str) -> Result<CallersResult> {
    let shard = ensure_shard(tag)?;
    let canonical = crate::util::normalize_type(type_name);
    // Resolve through alias map.
    let aliases: std::collections::BTreeMap<String, String> = shard.read_json("aliases")?;
    let mut resolved = canonical.clone();
    let mut seen = std::collections::HashSet::new();
    while let Some(next) = aliases.get(&resolved) {
        if !seen.insert(resolved.clone()) {
            break;
        }
        resolved = next.clone();
    }
    let reverse: Value = shard.read_json("reverse")?;
    let entry = reverse
        .get("consumers")
        .and_then(|c| c.get(&resolved))
        .cloned()
        .unwrap_or(Value::Null);
    let commands: Vec<String> = entry
        .get("commands")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let structs: Vec<String> = entry
        .get("structs")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    Ok(CallersResult {
        type_name: type_name.to_string(),
        canonical_name: resolved,
        commands,
        structs,
    })
}

// ---- find_dependencies ----------------------------------------------------

pub fn find_dependencies(name: &str, tag: &str) -> Result<DepGraph> {
    let shard = ensure_shard(tag)?;
    let functions: serde_json::Map<String, Value> = shard.read_json("functions")?;
    let f = functions.get(name).ok_or_else(|| anyhow!("function not found: {name}"))?;
    let info: FunctionInfo = serde_json::from_value(f.clone())?;

    // Parent handle chain: walk the FIRST handle-typed param's parent links,
    // pushing handle → root (matches Python's order). Stop after the first
    // handle param.
    let handles: serde_json::Map<String, Value> = shard.read_json("handles")?;
    let mut handle_chain: Vec<String> = Vec::new();
    for p in &info.params {
        if !handles.contains_key(&p.ty) {
            continue;
        }
        let mut cur = p.ty.clone();
        let mut seen = std::collections::HashSet::new();
        while !cur.is_empty() && !seen.contains(&cur) {
            seen.insert(cur.clone());
            handle_chain.push(cur.clone());
            cur = handles
                .get(&cur)
                .and_then(|v| v.get("parent"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
        }
        break;
    }

    // pNext chain: for each param whose type appears in `extended_by`, list
    // the structs that extend it.
    let reverse: Value = shard.read_json("reverse")?;
    let mut pnext: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for p in &info.params {
        if let Some(extenders) = reverse
            .get("extended_by")
            .and_then(|m| m.get(&p.ty))
            .and_then(|v| v.as_array())
        {
            let names: Vec<String> = extenders.iter().filter_map(|v| v.as_str().map(String::from)).collect();
            if !names.is_empty() {
                pnext.insert(p.ty.clone(), names);
            }
        }
    }

    // Required features vs. extensions: discriminator is exactly
    // `startswith("VK_VERSION_")`. Everything else in `available_in` is
    // an extension token.
    let required_features: Vec<String> = info
        .available_in
        .iter()
        .filter(|n| n.starts_with("VK_VERSION_"))
        .cloned()
        .collect();
    let required_extensions: Vec<String> = info
        .available_in
        .iter()
        .filter(|n| !n.starts_with("VK_VERSION_"))
        .cloned()
        .collect();

    // Transitive extension deps: BFS through `extension.requires_extensions`.
    let extensions_map: serde_json::Map<String, Value> = shard.read_json("extensions")?;
    let mut transitive: Vec<String> = Vec::new();
    let mut seen_ext: std::collections::HashSet<String> =
        required_extensions.iter().cloned().collect();
    let mut stack: Vec<String> = required_extensions.clone();
    while let Some(e) = stack.pop() {
        let Some(info) = extensions_map.get(&e) else { continue };
        if let Some(deps) = info.get("requires_extensions").and_then(|v| v.as_array()) {
            for d in deps {
                if let Some(s) = d.as_str() {
                    if seen_ext.insert(s.to_string()) {
                        transitive.push(s.to_string());
                        stack.push(s.to_string());
                    }
                }
            }
        }
    }

    let externsync: Vec<String> = info
        .params
        .iter()
        .filter_map(|p| match p.externsync.as_deref() {
            Some(v) if v != "false" => Some(p.name.clone()),
            _ => None,
        })
        .collect();

    Ok(DepGraph {
        function: name.to_string(),
        parent_handle_chain: handle_chain,
        required_features,
        required_extensions,
        transitive_extensions: transitive,
        pnext_chain: pnext,
        externsync_params: externsync,
        queues: info.queues.clone(),
        renderpass: info.renderpass.clone(),
        cmdbufferlevel: info.cmdbufferlevel.clone(),
        error_codes: info.error_codes.clone(),
    })
}

// ---- get_vuid -------------------------------------------------------------

pub fn get_vuid(vuid_id: &str, tag: &str) -> Result<VuidInfo> {
    let shard = ensure_shard(tag)?;
    let map: std::collections::BTreeMap<String, Value> = shard.read_json("vuids")?;
    let v = map
        .get(vuid_id)
        .ok_or_else(|| anyhow!("vuid not found: {vuid_id}"))?;
    let mut info: VuidInfo = serde_json::from_value(v.clone())?;
    // Splice `available_in` from the owning function or struct so callers can
    // tell which Vulkan version/extension the VUID applies to.
    let functions: serde_json::Map<String, Value> = shard.read_json("functions").unwrap_or_default();
    let structs: serde_json::Map<String, Value> = shard.read_json("structs").unwrap_or_default();
    let entry = functions.get(&info.entity).or_else(|| structs.get(&info.entity));
    if let Some(e) = entry {
        if let Some(av) = e.get("available_in").and_then(|v| v.as_array()) {
            info.available_in = av
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
    }
    Ok(info)
}

// ---- search_concept -------------------------------------------------------

pub fn search_concept(query: &str, tag: &str, k: usize, mode: &str) -> Result<Vec<SearchHit>> {
    let shard = ensure_shard(tag)?;
    match mode {
        "bm25" => {
            let bm25 = crate::search::bm25::Bm25::load(&shard.bm25_dir())
                .context("load BM25 index — has the shard been built with this Rust version?")?;
            Ok(bm25.search(query, k))
        }
        #[cfg(feature = "embed")]
        "embed" => {
            let emb_dir = shard.embeddings_dir();
            if !crate::search::embedding::index_usable(&emb_dir) {
                return Err(anyhow!(
                    "no usable embedding index at {} — run `vkquery index build --tag {tag} --force` (or unset VKQUERY_SKIP_EMBED) to populate it",
                    emb_dir.display()
                ));
            }
            crate::search::embedding::search_shard(&emb_dir, query, k)
        }
        #[cfg(feature = "embed")]
        "hybrid" => {
            let bm25 = crate::search::bm25::Bm25::load(&shard.bm25_dir())
                .context("load BM25 index for hybrid mode")?;
            let wide = k.saturating_mul(2).max(k + 5);
            let lexical = bm25.search(query, wide);
            // Mirror Python's `search_concept`: when embeddings are missing
            // or fail to load, warn and fall back to BM25-only RRF (which
            // degenerates to the BM25 ranking, deduped).
            let emb_dir = shard.embeddings_dir();
            let semantic = if crate::search::embedding::index_usable(&emb_dir) {
                match crate::search::embedding::search_shard(&emb_dir, query, wide) {
                    Ok(h) => h,
                    Err(e) => {
                        tracing::warn!(
                            "hybrid: embedding search failed ({e:?}); falling back to BM25-only"
                        );
                        Vec::new()
                    }
                }
            } else {
                tracing::warn!(
                    "hybrid: no usable embedding index at {} — falling back to BM25-only (rebuild with `vkquery index build --tag {tag} --force` to enable semantic results)",
                    emb_dir.display()
                );
                Vec::new()
            };
            Ok(crate::search::hybrid::rrf(&lexical, &semantic, k))
        }
        #[cfg(not(feature = "embed"))]
        "embed" | "hybrid" => Err(anyhow!(
            "search mode {mode:?} requires the `embed` cargo feature — rebuild with `--features embed`"
        )),
        other => Err(anyhow!("unknown search mode: {other:?}")),
    }
}
