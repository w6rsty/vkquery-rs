//! Orchestrator: ensure shard fresh, load vk.xml at the requested tag,
//! parse it, build every entity index, and write the shard.
//!
//! End-to-end pipeline: XML indices (functions / structs / handles /
//! enums / extensions / features / aliases / reverse) → explicit +
//! implicit VUIDs → BM25 corpus → optional BERT embeddings (gated by
//! `embed` feature and `VKQUERY_SKIP_EMBED`).

use anyhow::{Context, Result};

use crate::cache::{Cache, Shard};
use crate::docs_source::DocsSource;
use crate::git::TagReader;
use crate::index::{prose, reverse, vuid_explicit, vuid_implicit, xml_index};
use crate::search::bm25::{Bm25, Bm25Doc};
use crate::registry;

/// Per-shard JSON file names. Used by the freshness check — if any of these
/// is missing, the shard is considered stale and gets rebuilt.
pub const XML_INDEX_NAMES: &[&str] = &[
    "functions",
    "structs",
    "handles",
    "enums",
    "extensions",
    "features",
    "aliases",
    "reverse",
    "vuids",
];

/// Try common locations for `vk.xml` at a given ref — modern is `xml/vk.xml`;
/// pre-1.0.50ish tags used `src/spec/vk.xml`.
fn read_vk_xml(reader: &mut TagReader, refspec: &str) -> Result<Vec<u8>> {
    if let Ok(b) = reader.read_blob(refspec, "xml/vk.xml") {
        return Ok(b);
    }
    if let Ok(b) = reader.read_blob(refspec, "src/spec/vk.xml") {
        return Ok(b);
    }
    Err(anyhow::anyhow!(
        "vk.xml not found at {refspec} (tried xml/vk.xml and src/spec/vk.xml)"
    ))
}

pub fn build_shard(source: &DocsSource, cache: &Cache, tag: &str, force: bool) -> Result<Shard> {
    let shard = cache.shard_for(source, tag)?;
    if !force && cache.is_fresh(&shard, XML_INDEX_NAMES) {
        tracing::debug!("shard {tag} already fresh");
        return Ok(shard);
    }
    tracing::info!("building shard for {tag} (commit {})", &shard.commit_sha[..12.min(shard.commit_sha.len())]);

    let xml_bytes = {
        let mut reader = TagReader::open(source).context("open cat-file")?;
        read_vk_xml(&mut reader, tag)?
    };
    let xml = std::str::from_utf8(&xml_bytes).context("vk.xml is not utf-8")?;
    let reg = registry::parse_registry(xml).with_context(|| format!("parse vk.xml at {tag}"))?;
    let mut result = xml_index::build_all(&reg);
    let reverse_value = reverse::build_reverse(&reg, &result.aliases);

    // Extract explicit VUIDs from chapters/*.adoc at the same ref. Derived
    // implicit VUIDs are merged in below — explicit wins on collisions.
    // If chapter extraction fails (e.g. legacy tags without `chapters/`),
    // we proceed; functions/structs still get a `vuid_refs: []` field.
    let mut vuids = std::collections::BTreeMap::new();
    {
        match TagReader::open(source) {
            Ok(mut chapter_reader) => match vuid_explicit::extract_from_chapters(&mut chapter_reader, tag) {
                Ok(v) => vuids = v,
                Err(e) => tracing::warn!("explicit VUID extraction failed at {tag}: {e:?}"),
            },
            Err(e) => tracing::warn!("could not open chapter reader at {tag}: {e:?}"),
        }
    }

    // Derive implicit VUIDs from XML attributes. We need the `extended_by`
    // map for pNext extender chains — pull it from the reverse index.
    {
        let mut extended_by: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        if let Some(eb) = reverse_value.get("extended_by").and_then(|v| v.as_object()) {
            for (k, v) in eb {
                if let Some(arr) = v.as_array() {
                    extended_by.insert(
                        k.clone(),
                        arr.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
                    );
                }
            }
        }
        let implicit = vuid_implicit::derive_from_registry(&reg, &extended_by);
        // Merge: explicit wins on collisions (matches Python `merge` semantics).
        for (id, v) in implicit {
            vuids.entry(id).or_insert(v);
        }
    }

    // Splice `vuid_refs` arrays into functions / structs before writing them.
    // Take both values out (BTreeMap can't hand out two mutable borrows at once),
    // mutate, then put them back.
    {
        let mut functions_val = result.entities.remove("functions");
        let mut structs_val = result.entities.remove("structs");
        if let (Some(fv), Some(sv)) = (functions_val.as_mut(), structs_val.as_mut()) {
            if let (Some(f_obj), Some(s_obj)) = (fv.as_object_mut(), sv.as_object_mut()) {
                vuid_explicit::attach_vuid_refs(&vuids, f_obj, s_obj);
            }
        }
        if let Some(fv) = functions_val {
            result.entities.insert("functions", fv);
        }
        if let Some(sv) = structs_val {
            result.entities.insert("structs", sv);
        }
    }

    std::fs::create_dir_all(&shard.root)
        .with_context(|| format!("mkdir {}", shard.root.display()))?;
    for (name, value) in &result.entities {
        shard.write_json(name, value)?;
    }
    shard.write_json("aliases", &result.aliases)?;
    shard.write_json("reverse", &reverse_value)?;
    shard.write_json("vuids", &vuids)?;

    // Prose sections + BM25 lexical index for `search_concept` queries.
    let sections = {
        match TagReader::open(source) {
            Ok(mut chapter_reader) => prose::extract_sections(&mut chapter_reader, tag)
                .unwrap_or_else(|e| {
                    tracing::warn!("prose section extraction failed: {e:?}");
                    Vec::new()
                }),
            Err(e) => {
                tracing::warn!("could not open chapter reader for prose: {e:?}");
                Vec::new()
            }
        }
    };
    let mut bm25_docs: Vec<Bm25Doc> = Vec::with_capacity(sections.len() + vuids.len());
    for s in &sections {
        if s.text.is_empty() {
            continue;
        }
        let entity_hint = s.refpage_entities.first().cloned();
        let tokens = crate::search::bm25::tokenize(&s.text);
        bm25_docs.push(Bm25Doc {
            kind: "section".into(),
            source_id: s.section_id.clone(),
            source_file: Some(s.file.clone()),
            section_anchor: Some(s.section_id.clone()),
            entity_hint,
            text: s.text.clone(),
            tokens,
        });
    }
    for (vid, v) in &vuids {
        let text = v.get("text").and_then(|t| t.as_str()).unwrap_or("");
        if text.is_empty() {
            continue;
        }
        let tokens = crate::search::bm25::tokenize(text);
        bm25_docs.push(Bm25Doc {
            kind: "vuid".into(),
            source_id: vid.clone(),
            source_file: v.get("source_file").and_then(|s| s.as_str()).map(String::from),
            section_anchor: None,
            entity_hint: v.get("entity").and_then(|e| e.as_str()).map(String::from),
            text: text.to_string(),
            tokens,
        });
    }
    let bm25 = Bm25::from_docs(bm25_docs);
    if let Err(e) = bm25.save(&shard.bm25_dir()) {
        tracing::warn!("BM25 index write failed: {e:?}");
    }

    // Embeddings (optional). The first build pulls the bge-small model
    // from HuggingFace (~130MB) into ~/.cache/huggingface; later builds
    // are warm. Skip in `VKQUERY_SKIP_EMBED=1` so CI shards can elide
    // the download in environments without network.
    #[cfg(feature = "embed")]
    {
        let skip = std::env::var("VKQUERY_SKIP_EMBED").is_ok();
        if !skip {
            match crate::search::embedding::build_index(
                &sections,
                &vuids,
                &shard.embeddings_dir(),
                None,
            ) {
                Ok((n, dim, model)) => {
                    tracing::info!("embeddings: {n} vectors, dim={dim}, model={model}");
                }
                Err(e) => tracing::warn!("embedding build failed: {e:?}"),
            }
        }
    }

    let mut extra = std::collections::BTreeMap::new();
    extra.insert(
        "type_count".to_string(),
        serde_json::Value::Number((reg.types.len() as u64).into()),
    );
    extra.insert(
        "command_count".to_string(),
        serde_json::Value::Number((reg.commands.len() as u64).into()),
    );
    extra.insert(
        "extension_count".to_string(),
        serde_json::Value::Number((reg.extensions.len() as u64).into()),
    );
    shard.write_manifest(Some(extra))?;
    cache.update_index(&shard)?;
    tracing::info!(
        "shard built at {} ({} types, {} commands, {} extensions)",
        shard.root.display(),
        reg.types.len(),
        reg.commands.len(),
        reg.extensions.len()
    );
    Ok(shard)
}
