//! Explicit VUID extraction from `chapters/*.adoc`.
//!
//! Mirrors Python `vkquery.index.vuid_explicit` line-for-line:
//! - regex anchor `* [[VUID-entity-param-NNNNN]] text` + continuation lines,
//! - `ifdef::EXT[]` / `ifndef::EXT[]` / `endif::` guard stack,
//! - `[open,refpage='X']` refpage context with `--` block close,
//! - `:refpage: X` inline attribute,
//! - `include::{chapters}/commonvalidity/<X>.adoc[]` recursive resolution,
//! - `{refpage}` placeholder substitution from current refpage,
//! - dedup keeping the entry with the smallest `guard_extensions`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{json, Map, Value};

use crate::git::TagReader;
use crate::util::strip_asciidoc_markup;

const COMMON_PREFIX: &str = "chapters/commonvalidity/";

// ---- regex cache ----------------------------------------------------------

static VUID_ANCHOR: OnceLock<Regex> = OnceLock::new();
static REFPAGE_OPEN: OnceLock<Regex> = OnceLock::new();
static REFPAGE_ATTR: OnceLock<Regex> = OnceLock::new();
static INCLUDE_COMMON: OnceLock<Regex> = OnceLock::new();
static IFDEF: OnceLock<Regex> = OnceLock::new();
static IFNDEF: OnceLock<Regex> = OnceLock::new();
static ENDIF: OnceLock<Regex> = OnceLock::new();
static NEXT_BULLET: OnceLock<Regex> = OnceLock::new();
static BLOCK_DELIM: OnceLock<Regex> = OnceLock::new();

fn vuid_anchor() -> &'static Regex {
    VUID_ANCHOR.get_or_init(|| {
        Regex::new(r"^\s*\*\s+\[\[VUID-([A-Za-z0-9_{}]+)-([A-Za-z0-9_{}-]+)-(\d{5})\]\]\s*(.*)$")
            .unwrap()
    })
}
fn refpage_open() -> &'static Regex {
    REFPAGE_OPEN.get_or_init(|| Regex::new(r"^\[open\s*,\s*refpage='([^']+)'").unwrap())
}
fn refpage_attr_re() -> &'static Regex {
    REFPAGE_ATTR.get_or_init(|| Regex::new(r"^:refpage:\s+(\S+)").unwrap())
}
fn include_common() -> &'static Regex {
    INCLUDE_COMMON
        .get_or_init(|| Regex::new(r"^include::\{chapters\}/commonvalidity/([^\[\]]+)\.adoc\[\]").unwrap())
}
fn ifdef() -> &'static Regex {
    IFDEF.get_or_init(|| Regex::new(r"^ifdef::([A-Za-z0-9_,+]+)\[\]").unwrap())
}
fn ifndef() -> &'static Regex {
    IFNDEF.get_or_init(|| Regex::new(r"^ifndef::([A-Za-z0-9_,+]+)\[\]").unwrap())
}
fn endif() -> &'static Regex {
    ENDIF.get_or_init(|| Regex::new(r"^endif::([A-Za-z0-9_,+]*)\[\]").unwrap())
}
fn next_bullet() -> &'static Regex {
    NEXT_BULLET.get_or_init(|| Regex::new(r"^\s*\*\s+").unwrap())
}
fn block_delim() -> &'static Regex {
    BLOCK_DELIM.get_or_init(|| Regex::new(r"^(\*{4,}|----|====|--)\s*$").unwrap())
}

fn gate_extensions(token: &str) -> Vec<String> {
    token
        .split(|c: char| c == ',' || c == '+')
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

// ---- public entry ---------------------------------------------------------

/// Read every `chapters/**/*.adoc` blob at `refspec` and extract explicit
/// VUIDs. Returns a BTreeMap keyed by VUID id so JSON output is sorted.
pub fn extract_from_chapters(
    reader: &mut TagReader,
    refspec: &str,
) -> Result<BTreeMap<String, Value>> {
    let paths = reader
        .list_adoc(refspec, "chapters/")
        .context("list chapters")?;
    // Load every chapter blob into memory (matches Python which does the same).
    // Files are typically ≤ a few hundred KB and there are ~100 of them.
    let mut blobs: BTreeMap<String, String> = BTreeMap::new();
    for path in &paths {
        let bytes = reader
            .read_blob(refspec, path)
            .with_context(|| format!("read {path}"))?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        blobs.insert(path.clone(), text);
    }

    let common: HashMap<String, String> = blobs
        .iter()
        .filter(|(p, _)| p.starts_with(COMMON_PREFIX))
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();

    let mut out: BTreeMap<String, Value> = BTreeMap::new();
    for (path, text) in &blobs {
        if path.starts_with(COMMON_PREFIX) {
            continue;
        }
        scan_file(path, text, None, &common, &mut out);
    }
    Ok(out)
}

fn scan_file(
    source_file: &str,
    text: &str,
    refpage_attr: Option<&str>,
    common: &HashMap<String, String>,
    out: &mut BTreeMap<String, Value>,
) {
    let lines: Vec<&str> = text.lines().collect();
    let mut guard_stack: Vec<Vec<String>> = Vec::new();
    let mut cur_refpage: Option<String> = refpage_attr.map(String::from);
    let mut refpage_block_stack: Vec<(String, usize)> = Vec::new();
    let n = lines.len();
    let mut i = 0usize;

    while i < n {
        let line = lines[i];

        // ifdef / ifndef / endif
        if let Some(c) = ifdef().captures(line) {
            guard_stack.push(gate_extensions(c.get(1).unwrap().as_str()));
            i += 1;
            continue;
        }
        if ifndef().is_match(line) {
            guard_stack.push(Vec::new());
            i += 1;
            continue;
        }
        if endif().is_match(line) {
            if !guard_stack.is_empty() {
                guard_stack.pop();
            }
            i += 1;
            continue;
        }

        // refpage_open / refpage close
        if let Some(c) = refpage_open().captures(line) {
            let entity = c.get(1).unwrap().as_str().to_string();
            refpage_block_stack.push((entity.clone(), i));
            cur_refpage = Some(entity);
            i += 1;
            continue;
        }
        if line.trim() == "--" && !refpage_block_stack.is_empty() {
            refpage_block_stack.pop();
            cur_refpage = refpage_block_stack
                .last()
                .map(|(e, _)| e.clone())
                .or_else(|| refpage_attr.map(String::from));
            i += 1;
            continue;
        }
        if let Some(c) = refpage_attr_re().captures(line) {
            cur_refpage = Some(c.get(1).unwrap().as_str().to_string());
            i += 1;
            continue;
        }

        // include::{chapters}/commonvalidity/X.adoc[]
        if let Some(c) = include_common().captures(line) {
            let inc_path = format!("{COMMON_PREFIX}{}.adoc", c.get(1).unwrap().as_str());
            if let Some(inc_text) = common.get(&inc_path) {
                scan_file(&inc_path, inc_text, cur_refpage.as_deref(), common, out);
            }
            i += 1;
            continue;
        }

        // VUID anchor
        if let Some(c) = vuid_anchor().captures(line) {
            let block_entity =
                substitute_refpage(c.get(1).unwrap().as_str(), cur_refpage.as_deref());
            let param =
                substitute_refpage(c.get(2).unwrap().as_str(), cur_refpage.as_deref());
            let number = c.get(3).unwrap().as_str().to_string();
            if block_entity.is_empty() {
                // Anchors that reference {refpage} without a current refpage
                // get dropped, matching Python behavior.
                i += 1;
                continue;
            }
            let vuid = format!("VUID-{block_entity}-{param}-{number}");

            let mut text_parts: Vec<String> = Vec::new();
            let first_chunk = c.get(4).map(|m| m.as_str().trim()).unwrap_or("");
            if !first_chunk.is_empty() {
                text_parts.push(first_chunk.to_string());
            }
            i += 1;
            while i < n {
                let nxt = lines[i];
                if next_bullet().is_match(nxt) {
                    break;
                }
                if block_delim().is_match(nxt) {
                    break;
                }
                if ifdef().is_match(nxt) || ifndef().is_match(nxt) || endif().is_match(nxt) {
                    break;
                }
                let trimmed = nxt.trim();
                if !trimmed.is_empty() {
                    text_parts.push(trimmed.to_string());
                }
                i += 1;
            }
            let raw_text = text_parts.join(" ");
            let normalized = strip_asciidoc_markup(&raw_text);

            // guard_extensions = sorted unique union of currently open ifdefs
            let mut guard_set: BTreeSet<String> = BTreeSet::new();
            for stack in &guard_stack {
                for g in stack {
                    guard_set.insert(g.clone());
                }
            }
            let guard_flat: Vec<String> = guard_set.into_iter().collect();
            let cur_guard_len = out
                .get(&vuid)
                .and_then(|v| v.get("guard_extensions"))
                .and_then(|v| v.as_array())
                .map(|a| a.len())
                .unwrap_or(usize::MAX);

            if !out.contains_key(&vuid) || guard_flat.len() < cur_guard_len {
                let entry = json!({
                    "entity": block_entity,
                    "guard_extensions": guard_flat,
                    "id": vuid,
                    "kind": "explicit",
                    "param": param,
                    "source_file": source_file,
                    "text": normalized,
                });
                out.insert(vuid, entry);
            }
            continue;
        }

        i += 1;
    }
}

fn substitute_refpage(token: &str, refpage: Option<&str>) -> String {
    if !token.contains("{refpage}") {
        return token.to_string();
    }
    match refpage {
        None => String::new(),
        Some(r) => token.replace("{refpage}", r),
    }
}

// ---- back-linking ---------------------------------------------------------

/// Add a sorted `vuid_refs` array to each function and struct in-place,
/// listing the VUID ids whose `entity` matches.
pub fn attach_vuid_refs(
    vuids: &BTreeMap<String, Value>,
    functions: &mut Map<String, Value>,
    structs: &mut Map<String, Value>,
) {
    let mut by_entity: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (id, v) in vuids {
        if let Some(entity) = v.get("entity").and_then(|e| e.as_str()) {
            by_entity
                .entry(entity.to_string())
                .or_default()
                .insert(id.clone());
        }
    }

    for (fname, f_value) in functions.iter_mut() {
        let refs: Vec<String> = by_entity
            .get(fname)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        if let Some(obj) = f_value.as_object_mut() {
            obj.insert("vuid_refs".to_string(), json!(refs));
        }
    }
    for (sname, s_value) in structs.iter_mut() {
        let refs: Vec<String> = by_entity
            .get(sname)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        if let Some(obj) = s_value.as_object_mut() {
            obj.insert("vuid_refs".to_string(), json!(refs));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitute_refpage_replaces_placeholder() {
        assert_eq!(substitute_refpage("vkCmdDraw", Some("vkCmdDispatch")), "vkCmdDraw");
        assert_eq!(
            substitute_refpage("{refpage}", Some("vkCmdDraw")),
            "vkCmdDraw"
        );
        assert_eq!(substitute_refpage("{refpage}", None), "");
        assert_eq!(
            substitute_refpage("{refpage}-cmdpool", Some("vkCmdDraw")),
            "vkCmdDraw-cmdpool"
        );
    }

    #[test]
    fn gate_extensions_splits_on_comma_and_plus() {
        assert_eq!(gate_extensions("VK_KHR_a,VK_KHR_b"), vec!["VK_KHR_a", "VK_KHR_b"]);
        assert_eq!(gate_extensions("VK_KHR_a+VK_KHR_b"), vec!["VK_KHR_a", "VK_KHR_b"]);
        assert!(gate_extensions("").is_empty());
    }

    #[test]
    fn scan_file_picks_up_simple_vuid() {
        let mut out = BTreeMap::new();
        let text = r#"
[open,refpage='vkCmdDraw',desc='draw',type='protos']
--
some intro

  * [[VUID-vkCmdDraw-None-02700]] If a `pname:VkBuffer` is bound, it must: be valid.
  * Some other bullet that isn't a VUID

--
"#;
        scan_file("chapters/draw.adoc", text, None, &HashMap::new(), &mut out);
        assert!(out.contains_key("VUID-vkCmdDraw-None-02700"));
        let v = &out["VUID-vkCmdDraw-None-02700"];
        assert_eq!(v["entity"], "vkCmdDraw");
        assert_eq!(v["kind"], "explicit");
        assert!(v["text"].as_str().unwrap().contains("must"));
    }

    #[test]
    fn scan_file_ifdef_stack_tags_guards() {
        let mut out = BTreeMap::new();
        let text = r#"
[open,refpage='vkCmdFoo',desc='foo',type='protos']
--
ifdef::VK_KHR_a[]
  * [[VUID-vkCmdFoo-bar-00001]] guarded bullet
endif::VK_KHR_a[]
--
"#;
        scan_file("chapters/foo.adoc", text, None, &HashMap::new(), &mut out);
        let v = &out["VUID-vkCmdFoo-bar-00001"];
        assert_eq!(v["guard_extensions"].as_array().unwrap()[0], "VK_KHR_a");
    }

    #[test]
    fn scan_file_refpage_substitution_via_commonvalidity() {
        // Real Vulkan-Docs pattern: `[open,refpage='X']` pushes the entity,
        // then `--` immediately pops the refpage_block_stack. The chapter
        // re-sets the refpage with `:refpage: X` *inside* the block so it
        // survives across the include. Mirror that here.
        let mut common = HashMap::new();
        common.insert(
            "chapters/commonvalidity/draw.adoc".to_string(),
            r#"  * [[VUID-{refpage}-None-00010]] common rule"#.to_string(),
        );
        let mut out = BTreeMap::new();
        let text = r#"
[open,refpage='vkCmdDrawIndirect',desc='',type='protos']
--
:refpage: vkCmdDrawIndirect

include::{chapters}/commonvalidity/draw.adoc[]
--
"#;
        scan_file("chapters/dispatch.adoc", text, None, &common, &mut out);
        assert!(
            out.contains_key("VUID-vkCmdDrawIndirect-None-00010"),
            "got: {:?}",
            out.keys().collect::<Vec<_>>()
        );
    }
}
