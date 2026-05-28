//! Walk a parsed [`Registry`] → emit per-entity JSON values.
//!
//! Mirrors `vkquery.index.xml_index` exactly so shard JSONs are byte-stable
//! across Rust and Python implementations. JSON object keys end up sorted
//! because we build everything as `serde_json::Map` (BTreeMap-backed in the
//! default `serde_json` feature set).

use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::registry::{Command, Extension, Feature, Member, Param, Registry, Type};
use crate::util::extension_author;

const QUEUE_PREFIX: &str = "VK_QUEUE_";
const QUEUE_SUFFIX: &str = "_BIT";

/// `VK_QUEUE_GRAPHICS_BIT` → `graphics`. Pass-through on already-normalized
/// tokens. Mirrors Python `_norm_queue`.
fn norm_queue(token: &str) -> String {
    let t = token.trim();
    if t.starts_with(QUEUE_PREFIX) && t.ends_with(QUEUE_SUFFIX) {
        return t[QUEUE_PREFIX.len()..t.len() - QUEUE_SUFFIX.len()].to_ascii_lowercase();
    }
    t.to_ascii_lowercase()
}


/// `_is_vulkan`: true if this feature/extension applies to the `vulkan` api
/// (not vulkansc-only). The XML may carry either an `api=` or `supported=`
/// attribute.
fn applies_to_vulkan(api: &str, supported: &[String]) -> bool {
    if !api.is_empty() {
        return api.split(',').any(|s| s.trim() == "vulkan");
    }
    if !supported.is_empty() {
        return supported.iter().any(|s| s == "vulkan");
    }
    true
}

fn is_vulkan_feature(f: &Feature) -> bool {
    // Compositional sub-features (VK_BASE_VERSION_1_0, VK_GRAPHICS_VERSION_1_0, …)
    // carry `apitype="internal"` and are aggregated into the public VK_VERSION_N_M
    // by the spec build. Drop them from user-facing indices.
    if f.apitype.as_deref() == Some("internal") {
        return false;
    }
    applies_to_vulkan(&f.api, &[])
}

fn is_vulkan_extension(e: &Extension) -> bool {
    // Extensions don't have `api=`; they have `supported=`.
    applies_to_vulkan("", &e.supported)
}

// ---- alias bookkeeping ----------------------------------------------------

fn alias_back(aliases_fwd: &BTreeMap<String, String>) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (src, dst) in aliases_fwd {
        out.entry(dst.clone()).or_default().push(src.clone());
    }
    for v in out.values_mut() {
        v.sort();
    }
    out
}

// ---- feature_origin / available_in ----------------------------------------

#[derive(Default, Debug)]
struct OriginEntry {
    feature_origin: Option<String>,
    available_in: Vec<String>,
}

#[derive(Default, Debug)]
struct OriginMaps {
    commands: BTreeMap<String, OriginEntry>,
    types: BTreeMap<String, OriginEntry>,
    enums: BTreeMap<String, OriginEntry>,
}

impl OriginMaps {
    fn absorb(map: &mut BTreeMap<String, OriginEntry>, name: &str, container: &str) {
        let entry = map.entry(name.to_string()).or_default();
        if entry.feature_origin.is_none() {
            entry.feature_origin = Some(container.to_string());
        }
        if !entry.available_in.iter().any(|c| c == container) {
            entry.available_in.push(container.to_string());
        }
    }
}

fn collect_origin(reg: &Registry) -> OriginMaps {
    let mut out = OriginMaps::default();
    // Public features first so promotion wins over the original extension.
    // We iterate in XML-declaration order (features_order) — alphabetical
    // sort would attribute multi-extension entities to the wrong source.
    // For each public feature, walk its `depends` chain through internal
    // sub-features (apitype="internal") and absorb their provides_* lists
    // too — that's how vkCmdDraw, declared inside VK_GRAPHICS_VERSION_1_0,
    // gets attributed to the public VK_VERSION_1_0.
    for fname in &reg.features_order {
        let Some(f) = reg.features.get(fname) else { continue };
        if !is_vulkan_feature(f) {
            continue;
        }
        let (cmds, types_, enums_) = collect_rolled_up_provides(reg, fname);
        for n in &cmds {
            OriginMaps::absorb(&mut out.commands, n, fname);
        }
        for n in &types_ {
            OriginMaps::absorb(&mut out.types, n, fname);
        }
        for n in &enums_ {
            OriginMaps::absorb(&mut out.enums, n, fname);
        }
    }
    for ename in &reg.extensions_order {
        let Some(e) = reg.extensions.get(ename) else { continue };
        if !is_vulkan_extension(e) {
            continue;
        }
        for n in &e.provides_commands {
            OriginMaps::absorb(&mut out.commands, n, ename);
        }
        for n in &e.provides_types {
            OriginMaps::absorb(&mut out.types, n, ename);
        }
        for n in &e.provides_enums {
            OriginMaps::absorb(&mut out.enums, n, ename);
        }
    }
    out
}

fn origin_to_pair(map: &BTreeMap<String, OriginEntry>, name: &str) -> (Option<String>, Vec<String>) {
    map.get(name)
        .map(|o| (o.feature_origin.clone(), o.available_in.clone()))
        .unwrap_or_else(|| (None, vec![]))
}

// ---- functions ------------------------------------------------------------

fn param_to_value(p: &Param) -> Value {
    json!({
        "name": p.name,
        "type": p.ty,
        "optional": p.optional,
        "len": p.len.clone().map(Value::String).unwrap_or(Value::Null),
        "externsync": p.externsync.clone().map(Value::String).unwrap_or(Value::Null),
        "noautovalidity": p.noautovalidity,
        "const": p.is_const,
        "pointer_depth": p.pointer_depth,
    })
}

fn member_to_value(m: &Member) -> Value {
    json!({
        "name": m.name,
        "type": m.ty,
        "optional": m.optional,
        "len": m.len.clone().map(Value::String).unwrap_or(Value::Null),
        "externsync": m.externsync.clone().map(Value::String).unwrap_or(Value::Null),
        "noautovalidity": m.noautovalidity,
        "values": m.values.clone().map(Value::String).unwrap_or(Value::Null),
        "const": m.is_const,
        "pointer_depth": m.pointer_depth,
    })
}

/// Walk the forward alias map to a fixpoint and return the canonical name.
fn resolve_alias<'a>(aliases_fwd: &'a BTreeMap<String, String>, name: &'a str) -> String {
    let mut cur = name.to_string();
    let mut seen = std::collections::HashSet::new();
    while let Some(next) = aliases_fwd.get(&cur) {
        if !seen.insert(cur.clone()) {
            break;
        }
        cur = next.clone();
    }
    cur
}

fn build_functions(
    reg: &Registry,
    origin: &OriginMaps,
    aliases_fwd: &BTreeMap<String, String>,
    aliases_back: &BTreeMap<String, Vec<String>>,
) -> Value {
    let mut out: Map<String, Value> = Map::new();
    for (name, cmd) in &reg.commands {
        // Aliased commands have empty params in the parser output. Resolve
        // to the canonical command and copy its signature — Python's reg.py
        // does this resolution at load time.
        let canonical_name = resolve_alias(aliases_fwd, name);
        let source = reg.commands.get(&canonical_name).unwrap_or(cmd);
        let (feature_origin, available_in) = origin_to_pair(&origin.commands, name);
        let params: Vec<Value> = source.params.iter().map(param_to_value).collect();
        let aliases = aliases_back.get(name).cloned().unwrap_or_default();
        out.insert(
            name.clone(),
            json!({
                "name": name,
                "return_type": source.return_type,
                "params": params,
                "success_codes": source.success_codes,
                "error_codes": source.error_codes,
                "queues": source.queues.iter().map(|q| norm_queue(q)).collect::<Vec<_>>(),
                "renderpass": source.renderpass.clone().map(Value::String).unwrap_or(Value::Null),
                "cmdbufferlevel": source.cmdbufferlevel,
                "tasks": source.tasks,
                "feature_origin": feature_origin.map(Value::String).unwrap_or(Value::Null),
                "available_in": available_in,
                "aliases": aliases,
                "aliased_from": aliases_fwd.get(name).cloned().map(Value::String).unwrap_or(Value::Null),
            }),
        );
    }
    Value::Object(out)
}

// ---- structs --------------------------------------------------------------

fn build_structs(
    reg: &Registry,
    origin: &OriginMaps,
    aliases_fwd: &BTreeMap<String, String>,
    aliases_back: &BTreeMap<String, Vec<String>>,
) -> Value {
    let mut out: Map<String, Value> = Map::new();
    for (name, ty) in &reg.types {
        if ty.category != "struct" && ty.category != "union" {
            continue;
        }
        // Unlike commands, we do NOT expand aliased structs here — we use
        // this type's own members verbatim. Aliased struct entries
        // therefore end up with `members: []`; the API layer
        // (`api::get_struct`) resolves the alias and re-reads the
        // canonical struct's data on the way out.
        let (feature_origin, available_in) = origin_to_pair(&origin.types, name);
        let members: Vec<Value> = ty.members.iter().map(member_to_value).collect();
        let aliases = aliases_back.get(name).cloned().unwrap_or_default();
        out.insert(
            name.clone(),
            json!({
                "name": name,
                "category": ty.category,
                "returnedonly": ty.returnedonly,
                "structextends": ty.structextends,
                "members": members,
                "feature_origin": feature_origin.map(Value::String).unwrap_or(Value::Null),
                "available_in": available_in,
                "aliases": aliases,
                "aliased_from": aliases_fwd.get(name).cloned().map(Value::String).unwrap_or(Value::Null),
            }),
        );
    }
    Value::Object(out)
}

// ---- handles --------------------------------------------------------------

fn build_handles(
    reg: &Registry,
    aliases_fwd: &BTreeMap<String, String>,
    aliases_back: &BTreeMap<String, Vec<String>>,
) -> Value {
    let mut out: Map<String, Value> = Map::new();
    for (name, ty) in &reg.types {
        if ty.category != "handle" {
            continue;
        }
        let aliases = aliases_back.get(name).cloned().unwrap_or_default();
        out.insert(
            name.clone(),
            json!({
                "name": name,
                "parent": ty.parent.clone().map(Value::String).unwrap_or(Value::Null),
                "dispatchable": ty.dispatchable,
                "objtypeenum": ty.objtypeenum.clone().map(Value::String).unwrap_or(Value::Null),
                "aliases": aliases,
                "aliased_from": aliases_fwd.get(name).cloned().map(Value::String).unwrap_or(Value::Null),
            }),
        );
    }
    Value::Object(out)
}

// ---- enums ----------------------------------------------------------------

fn build_enums(
    reg: &Registry,
    aliases_fwd: &BTreeMap<String, String>,
    aliases_back: &BTreeMap<String, Vec<String>>,
) -> Value {
    // Group extension-introduced enumerants by parent group so we can append
    // them after the in-place values. Some entries appear multiple times
    // across extensions (e.g. KHR + EXT both contribute the same alias) —
    // dedupe by (group, name).
    let mut ext_values: BTreeMap<&str, Vec<&crate::registry::EnumExtension>> = BTreeMap::new();
    for ee in &reg.enum_extensions {
        ext_values.entry(ee.extends.as_str()).or_default().push(ee);
    }
    let mut out: Map<String, Value> = Map::new();
    for (name, en) in &reg.enums {
        let mut values: Vec<Value> = Vec::new();
        for v in &en.values {
            values.push(json!({
                "name": v.name,
                "value": v.value.clone().map(Value::String).unwrap_or(Value::Null),
                "bitpos": v.bitpos.clone().map(Value::String).unwrap_or(Value::Null),
                "alias": v.alias.clone().map(Value::String).unwrap_or(Value::Null),
                "comment": v.comment.clone().map(Value::String).unwrap_or(Value::Null),
            }));
        }
        if let Some(extras) = ext_values.get(name.as_str()) {
            // Python doesn't dedupe extension-introduced enumerants — the
            // same name can appear twice if two extensions both contribute
            // it. Mirror that exactly.
            for ee in extras {
                values.push(json!({
                    "name": ee.name,
                    "value": ee.value.clone().map(Value::String).unwrap_or(Value::Null),
                    "bitpos": ee.bitpos.clone().map(Value::String).unwrap_or(Value::Null),
                    "alias": ee.alias.clone().map(Value::String).unwrap_or(Value::Null),
                    "comment": ee.comment.clone().map(Value::String).unwrap_or(Value::Null),
                }));
            }
        }
        let aliases = aliases_back.get(name).cloned().unwrap_or_default();
        out.insert(
            name.clone(),
            json!({
                "name": name,
                "type": if en.kind.is_empty() { "enum".into() } else { en.kind.clone() },
                "bitwidth": en.bitwidth,
                "values": values,
                "aliases": aliases,
                "aliased_from": aliases_fwd.get(name).cloned().map(Value::String).unwrap_or(Value::Null),
            }),
        );
    }
    Value::Object(out)
}

// ---- extensions -----------------------------------------------------------

fn build_extensions(reg: &Registry) -> Value {
    let depends_re = Regex::new(r"VK_[A-Za-z0-9_]+").unwrap();
    let mut out: Map<String, Value> = Map::new();
    for (name, ext) in &reg.extensions {
        if !is_vulkan_extension(ext) {
            continue;
        }
        let depends = ext.depends.as_deref().unwrap_or("");
        let mut req_extensions: BTreeSet<String> = BTreeSet::new();
        let mut req_core: Option<String> = None;
        for m in depends_re.find_iter(depends) {
            let token = m.as_str().to_string();
            if token.starts_with("VK_VERSION_") {
                if req_core.is_none() {
                    req_core = Some(token);
                }
            } else {
                req_extensions.insert(token);
            }
        }
        let status = if ext.obsoletedby.is_some() {
            "obsoleted"
        } else if ext.deprecatedby.is_some() {
            "deprecated"
        } else if ext.promotedto.is_some() {
            "promoted"
        } else {
            "active"
        };
        let provides_commands = sorted_dedup(&ext.provides_commands);
        let provides_types = sorted_dedup(&ext.provides_types);
        let provides_enums = sorted_dedup(&ext.provides_enums);
        let author = ext.author.clone().or_else(|| extension_author(name));
        out.insert(
            name.clone(),
            json!({
                "name": name,
                "number": ext.number,
                "type": ext.ty.clone().map(Value::String).unwrap_or(Value::Null),
                "author": author.map(Value::String).unwrap_or(Value::Null),
                "contact": ext.contact.clone().map(Value::String).unwrap_or(Value::Null),
                "supported": ext.supported,
                "depends": ext.depends.clone().map(Value::String).unwrap_or(Value::Null),
                "requires_extensions": req_extensions.into_iter().collect::<Vec<_>>(),
                "requires_core": req_core.map(Value::String).unwrap_or(Value::Null),
                "promotedto": ext.promotedto.clone().map(Value::String).unwrap_or(Value::Null),
                "deprecatedby": ext.deprecatedby.clone().map(Value::String).unwrap_or(Value::Null),
                "obsoletedby": ext.obsoletedby.clone().map(Value::String).unwrap_or(Value::Null),
                "status": status,
                "provides_commands": provides_commands,
                "provides_types": provides_types,
                "provides_enums": provides_enums,
            }),
        );
    }
    Value::Object(out)
}

// ---- features -------------------------------------------------------------

fn build_features(reg: &Registry) -> Value {
    let mut out: Map<String, Value> = Map::new();
    for (name, f) in &reg.features {
        if !is_vulkan_feature(f) {
            continue;
        }
        let (cmds, types_, enums_) = collect_rolled_up_provides(reg, name);
        out.insert(
            name.clone(),
            json!({
                "name": name,
                "number": f.number,
                "depends": f.depends.clone().map(Value::String).unwrap_or(Value::Null),
                "provides_commands": sorted_dedup(&cmds),
                "provides_types": sorted_dedup(&types_),
                "provides_enums": sorted_dedup(&enums_),
            }),
        );
    }
    Value::Object(out)
}

/// Walk `depends` from `start_name`, accumulating `provides_*` from every
/// internal sub-feature reached. `depends=` can be comma-separated (e.g.
/// `VK_VERSION_1_3,VK_GRAPHICS_VERSION_1_4`), so this is a BFS, not a chain
/// walk. We stop at public-feature boundaries — only follow links that land
/// on `apitype="internal"`.
fn collect_rolled_up_provides(
    reg: &Registry,
    start_name: &str,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut cmds: Vec<String> = Vec::new();
    let mut types_: Vec<String> = Vec::new();
    let mut enums_: Vec<String> = Vec::new();
    // Only roll up internal sub-features whose `number=` matches the public
    // feature's. Otherwise the depends chain (VK_GRAPHICS_VERSION_1_4 →
    // VK_GRAPHICS_VERSION_1_3 → … → VK_GRAPHICS_VERSION_1_0) would attribute
    // 1.0's commands to 1.4 too.
    let Some(start_f) = reg.features.get(start_name) else { return (cmds, types_, enums_) };
    let target_number = start_f.number.clone();
    let mut seen = std::collections::HashSet::new();
    let mut queue: Vec<String> = vec![start_name.to_string()];
    while let Some(n) = queue.pop() {
        if !seen.insert(n.clone()) {
            continue;
        }
        let Some(f) = reg.features.get(&n) else { continue };
        cmds.extend(f.provides_commands.iter().cloned());
        types_.extend(f.provides_types.iter().cloned());
        enums_.extend(f.provides_enums.iter().cloned());
        if let Some(d) = f.depends.as_ref() {
            for tok in d.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                if let Some(next) = reg.features.get(tok) {
                    if next.apitype.as_deref() == Some("internal") && next.number == target_number {
                        queue.push(tok.to_string());
                    }
                }
            }
        }
    }
    (cmds, types_, enums_)
}

fn sorted_dedup(items: &[String]) -> Vec<String> {
    let mut s: BTreeSet<String> = BTreeSet::new();
    for i in items {
        s.insert(i.clone());
    }
    s.into_iter().collect()
}

// ---- public orchestrator --------------------------------------------------

pub struct BuildResult {
    /// "functions", "structs", "handles", "enums", "extensions", "features"
    pub entities: BTreeMap<&'static str, Value>,
    /// Alias forward map (alias → canonical).
    pub aliases: BTreeMap<String, String>,
}

pub fn build_all(reg: &Registry) -> BuildResult {
    let aliases_fwd = reg.aliases.clone();
    let aliases_back = alias_back(&aliases_fwd);
    let origin = collect_origin(reg);
    let mut entities: BTreeMap<&'static str, Value> = BTreeMap::new();
    entities.insert("functions", build_functions(reg, &origin, &aliases_fwd, &aliases_back));
    entities.insert("structs", build_structs(reg, &origin, &aliases_fwd, &aliases_back));
    entities.insert("handles", build_handles(reg, &aliases_fwd, &aliases_back));
    entities.insert("enums", build_enums(reg, &aliases_fwd, &aliases_back));
    entities.insert("extensions", build_extensions(reg));
    entities.insert("features", build_features(reg));
    BuildResult { entities, aliases: aliases_fwd }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn norm_queue_matches_python() {
        assert_eq!(norm_queue("VK_QUEUE_GRAPHICS_BIT"), "graphics");
        assert_eq!(norm_queue("VK_QUEUE_COMPUTE_BIT"), "compute");
        assert_eq!(norm_queue("graphics"), "graphics");
        assert_eq!(norm_queue(" GRAPHICS "), "graphics");
    }
}

// Silence unused-import warnings on `Type` and `Command`/`Extension` in
// release configurations that elide some helpers.
#[allow(dead_code)]
fn _typecheck(_: &Type, _: &Command, _: &Extension, _: &Feature) {}
