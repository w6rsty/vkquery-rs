//! Single-pass reverse indices: consumers + extended_by.
//!
//! Mirrors `vkquery.index.reverse.build_reverse`.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::registry::Registry;

/// Walk `aliases_fwd` transitively to find the canonical name. Bounded by an
/// in-loop seen-set to defend against cycles in malformed XML.
fn canonical(aliases_fwd: &BTreeMap<String, String>, name: &str) -> String {
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

/// Produce a single JSON value of shape:
/// ```text
/// {
///   "consumers":   { "VkImage": { "commands": ["vkCmd…"], "structs": ["Vk…"] }, … },
///   "extended_by": { "VkImageCreateInfo": ["VkExternal…"], … }
/// }
/// ```
pub fn build_reverse(reg: &Registry, aliases_fwd: &BTreeMap<String, String>) -> Value {
    let mut consumers: BTreeMap<String, (BTreeMap<String, ()>, BTreeMap<String, ()>)> =
        BTreeMap::new();
    let mut extended_by: BTreeMap<String, BTreeMap<String, ()>> = BTreeMap::new();

    for (cmd_name, cmd) in &reg.commands {
        for p in &cmd.params {
            let t = canonical(aliases_fwd, &p.ty);
            if t.is_empty() {
                continue;
            }
            consumers
                .entry(t)
                .or_default()
                .0
                .insert(cmd_name.clone(), ());
        }
    }

    for (tname, ty) in &reg.types {
        if ty.category != "struct" && ty.category != "union" {
            continue;
        }
        for m in &ty.members {
            let t = canonical(aliases_fwd, &m.ty);
            if t.is_empty() {
                continue;
            }
            consumers
                .entry(t)
                .or_default()
                .1
                .insert(tname.clone(), ());
        }
        for ext in &ty.structextends {
            let ext = ext.trim();
            if ext.is_empty() {
                continue;
            }
            extended_by.entry(ext.to_string()).or_default().insert(tname.clone(), ());
        }
    }

    let mut consumers_out: Map<String, Value> = Map::new();
    for (k, (cmds, structs)) in consumers {
        consumers_out.insert(
            k,
            json!({
                "commands": cmds.keys().cloned().collect::<Vec<_>>(),
                "structs": structs.keys().cloned().collect::<Vec<_>>(),
            }),
        );
    }
    let mut extended_by_out: Map<String, Value> = Map::new();
    for (k, v) in extended_by {
        extended_by_out.insert(k, Value::Array(v.keys().cloned().map(Value::String).collect()));
    }

    json!({
        "consumers": consumers_out,
        "extended_by": extended_by_out,
    })
}
