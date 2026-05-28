//! roxmltree-backed `vk.xml` parser → owned [`Registry`].
//!
//! Mirrors the surface area that `scripts/reg.py` exposes to
//! `vkquery.index.xml_index` (Python). The output structs in
//! [`super::schema`] are populated with owned `String`s so downstream code
//! never has to keep the `roxmltree::Document` alive.

use anyhow::{anyhow, Result};
use roxmltree::Node;

use super::legacy;
use super::schema::*;

/// Parse vk.xml text → fully-populated [`Registry`].
pub fn parse_registry(xml: &str) -> Result<Registry> {
    let xml = legacy::repair(xml);
    let doc = roxmltree::Document::parse(&xml).map_err(|e| anyhow!("xml parse: {e}"))?;
    let root = doc.root_element();
    if root.tag_name().name() != "registry" {
        return Err(anyhow!("expected <registry> root, found <{}>", root.tag_name().name()));
    }
    let mut reg = Registry::default();
    for child in root.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "types" => parse_types(child, &mut reg),
            "commands" => parse_commands(child, &mut reg),
            "enums" => parse_enums_block(child, &mut reg),
            "feature" => parse_feature(child, &mut reg),
            "extensions" => parse_extensions(child, &mut reg),
            _ => {} // comment, vendorids, tags, platforms, formats, spirvextensions, …
        }
    }
    Ok(reg)
}

// ---- helpers ---------------------------------------------------------------

fn attr<'a>(n: &'a Node, key: &str) -> Option<&'a str> {
    n.attribute(key)
}

fn attr_owned(n: &Node, key: &str) -> Option<String> {
    n.attribute(key).map(str::to_string)
}

fn attr_bool(n: &Node, key: &str) -> bool {
    matches!(n.attribute(key), Some(v) if v != "false" && !v.is_empty())
}

/// Concatenate this node's descendant text. Mirrors Python's
/// `"".join(elem.itertext())` — preserves order, no separator.
fn inner_text(n: Node) -> String {
    let mut s = String::new();
    inner_text_into(n, &mut s);
    s
}

fn inner_text_into(n: Node, out: &mut String) {
    for c in n.descendants() {
        if c.is_text() {
            if let Some(t) = c.text() {
                out.push_str(t);
            }
        }
    }
    // `descendants()` includes `n` itself once; for elements that's a no-op
    // since elements have no `text()` of their own at the node level.
    let _ = out; // satisfy clippy if unused
}

// `Node` is `Copy` (it's a small reference wrapper), so this can take it by
// value and return a Node borrowing from the same document, side-stepping
// closure-lifetime headaches.
fn child<'a, 'input>(n: Node<'a, 'input>, tag: &str) -> Option<Node<'a, 'input>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == tag)
}

fn children_with_tag<'a, 'b>(
    n: Node<'a, 'b>,
    tag: &'a str,
) -> impl Iterator<Item = Node<'a, 'b>> + 'a {
    n.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == tag)
}

fn split_csv(v: Option<&str>) -> Vec<String> {
    let Some(v) = v else { return vec![] };
    v.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

// ---- <type> elements -------------------------------------------------------

fn parse_types(types_node: Node, reg: &mut Registry) {
    for t in children_with_tag(types_node, "type") {
        let name = type_name(&t);
        let Some(name) = name else { continue };
        let category = attr(&t, "category").unwrap_or("").to_string();
        let alias = attr_owned(&t, "alias");
        if let Some(a) = &alias {
            reg.aliases.insert(name.clone(), a.clone());
        }
        let mut ty = Type {
            name: name.clone(),
            category: category.clone(),
            parent: attr_owned(&t, "parent"),
            objtypeenum: attr_owned(&t, "objtypeenum"),
            returnedonly: attr_bool(&t, "returnedonly"),
            structextends: split_csv(attr(&t, "structextends")),
            alias: alias.clone(),
            members: vec![],
            dispatchable: false,
            raw: inner_text(t),
            // 64-bit bitmasks declare their flag enum via `bitvalues=`
            // (e.g. `VkAccessFlags2 bitvalues="VkAccessFlagBits2"`). 32-bit
            // bitmasks use `requires=`. Prefer bitvalues so implicit VUIDs
            // emit the "valid combination of …" form for either.
            requires: attr_owned(&t, "bitvalues").or_else(|| attr_owned(&t, "requires")),
            source_xml: "xml/vk.xml".into(),
        };
        match category.as_str() {
            "struct" | "union" => {
                // Skip members marked `api="vulkansc"` (etc.) that aren't part
                // of the vulkan API surface. vk.xml occasionally splits a
                // member into two variants — one for `api="vulkan,vulkanbase"`
                // (often with `optional="true"`) and one for `api="vulkansc"`
                // (often required). Keeping both would double-emit member
                // VUIDs and pick up the wrong `optional` for arraylength.
                ty.members = children_with_tag(t, "member")
                    .filter(|m| {
                        m.attribute("api")
                            .map(|api| api.split(',').any(|s| {
                                let s = s.trim();
                                s == "vulkan" || s == "vulkanbase"
                            }))
                            .unwrap_or(true)
                    })
                    .map(|m| parse_param(m, true))
                    .collect();
            }
            "handle" => {
                // Inner text contains either `VK_DEFINE_HANDLE(...)` or
                // `VK_DEFINE_NON_DISPATCHABLE_HANDLE(...)`.
                ty.dispatchable = !ty.raw.contains("VK_DEFINE_NON_DISPATCHABLE_HANDLE");
            }
            _ => {}
        }
        reg.types.insert(name, ty);
    }
}

/// `<type>` may have its name as an attribute *or* in a `<name>` child
/// element (handles, structs) — or wrapped in a `<proto>` for funcpointers.
fn type_name(t: &Node) -> Option<String> {
    if let Some(n) = t.attribute("name") {
        return Some(n.to_string());
    }
    if let Some(n_elem) = child(*t, "name") {
        if let Some(txt) = n_elem.text() {
            return Some(txt.to_string());
        }
    }
    if let Some(proto) = child(*t, "proto") {
        if let Some(n_elem) = child(proto, "name") {
            if let Some(txt) = n_elem.text() {
                return Some(txt.to_string());
            }
        }
    }
    None
}

/// Extract a `<param>` or `<member>` element. `is_member=true` toggles the
/// `values=` attribute capture.
fn parse_param(p: Node, is_member: bool) -> Member {
    let raw = inner_text(p);
    let pointer_depth = raw.matches('*').count() as i32;
    let is_const = raw.contains("const");
    let ty_text = child(p, "type")
        .and_then(|t| t.text())
        .map(crate::util::normalize_type)
        .unwrap_or_default();
    let name = child(p, "name").and_then(|t| t.text()).unwrap_or("").trim().to_string();
    Member {
        name,
        ty: ty_text,
        optional: attr_bool(&p, "optional"),
        len: attr_owned(&p, "len"),
        externsync: attr_owned(&p, "externsync"),
        noautovalidity: attr_bool(&p, "noautovalidity"),
        values: if is_member { attr_owned(&p, "values") } else { None },
        selector: if is_member { attr_owned(&p, "selector") } else { None },
        selection: if is_member { attr_owned(&p, "selection") } else { None },
        is_const,
        pointer_depth,
        raw_decl: raw,
    }
}

fn parse_param_as_command_param(p: Node) -> Param {
    let m = parse_param(p, false);
    Param {
        name: m.name,
        ty: m.ty,
        optional: m.optional,
        len: m.len,
        externsync: m.externsync,
        noautovalidity: m.noautovalidity,
        is_const: m.is_const,
        pointer_depth: m.pointer_depth,
        raw_decl: m.raw_decl,
    }
}

// ---- <commands>/<command> --------------------------------------------------

fn parse_commands(cmds: Node, reg: &mut Registry) {
    for c in children_with_tag(cmds, "command") {
        // Aliased command shorthand: `<command name="X" alias="Y"/>`.
        if let (Some(name), Some(target)) = (attr_owned(&c, "name"), attr_owned(&c, "alias")) {
            reg.aliases.insert(name.clone(), target.clone());
            reg.commands.insert(
                name.clone(),
                Command {
                    name,
                    alias: Some(target),
                    ..Default::default()
                },
            );
            continue;
        }
        let proto = child(c, "proto");
        let return_type = proto
            .and_then(|p| child(p, "type"))
            .and_then(|t| t.text())
            .map(crate::util::normalize_type)
            .unwrap_or_default();
        let cmd_name = proto
            .and_then(|p| child(p, "name"))
            .and_then(|t| t.text())
            .unwrap_or("")
            .trim()
            .to_string();
        if cmd_name.is_empty() {
            continue;
        }
        let cmd = Command {
            name: cmd_name.clone(),
            return_type,
            params: children_with_tag(c, "param")
                .filter(|p| {
                    p.attribute("api")
                        .map(|api| api.split(',').any(|s| {
                            let s = s.trim();
                            s == "vulkan" || s == "vulkanbase"
                        }))
                        .unwrap_or(true)
                })
                .map(parse_param_as_command_param)
                .collect(),
            success_codes: split_csv(attr(&c, "successcodes")),
            error_codes: split_csv(attr(&c, "errorcodes")),
            queues: split_csv(attr(&c, "queues")),
            renderpass: attr_owned(&c, "renderpass"),
            cmdbufferlevel: split_csv(attr(&c, "cmdbufferlevel")),
            tasks: split_csv(attr(&c, "tasks")),
            allow_no_queues: attr_bool(&c, "allownoqueues"),
            alias: None,
            raw_decl: inner_text(c),
        };
        reg.commands.insert(cmd_name, cmd);
    }
}

// ---- top-level <enums name="..."> blocks ----------------------------------

fn parse_enums_block(en: Node, reg: &mut Registry) {
    let Some(name) = attr_owned(&en, "name") else { return };
    let kind = attr(&en, "type").unwrap_or("").to_string();
    let bitwidth = attr(&en, "bitwidth")
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(32);
    let mut block = Enums {
        name: name.clone(),
        kind,
        bitwidth,
        values: vec![],
    };
    for e in children_with_tag(en, "enum") {
        let Some(ename) = attr_owned(&e, "name") else { continue };
        let alias = attr_owned(&e, "alias");
        if let Some(a) = &alias {
            reg.aliases.insert(ename.clone(), a.clone());
        }
        block.values.push(EnumValue {
            name: ename,
            value: attr_owned(&e, "value"),
            bitpos: attr_owned(&e, "bitpos"),
            alias,
            comment: attr_owned(&e, "comment"),
        });
    }
    reg.enums.insert(name, block);
}

// ---- <feature> -------------------------------------------------------------

fn parse_feature(f: Node, reg: &mut Registry) {
    let Some(name) = attr_owned(&f, "name") else { return };
    let mut feature = Feature {
        name: name.clone(),
        number: attr(&f, "number").unwrap_or("").to_string(),
        api: attr(&f, "api").unwrap_or("").to_string(),
        apitype: attr_owned(&f, "apitype"),
        depends: attr_owned(&f, "depends"),
        ..Default::default()
    };
    collect_require_blocks(
        f,
        &mut feature.provides_commands,
        &mut feature.provides_types,
        &mut feature.provides_enums,
        Some(&name),
        &mut reg.enum_extensions,
        &mut reg.aliases,
    );
    reg.features_order.push(name.clone());
    reg.features.insert(name, feature);
}

// ---- <extensions>/<extension> ----------------------------------------------

fn parse_extensions(exts: Node, reg: &mut Registry) {
    for e in children_with_tag(exts, "extension") {
        let Some(name) = attr_owned(&e, "name") else { continue };
        let number = attr(&e, "number").and_then(|s| s.parse::<i32>().ok());
        let mut ext = Extension {
            name: name.clone(),
            number,
            ty: attr_owned(&e, "type"),
            author: attr_owned(&e, "author"),
            contact: attr_owned(&e, "contact"),
            supported: split_csv(attr(&e, "supported")),
            depends: attr_owned(&e, "depends"),
            requires_extensions: vec![],
            requires_core: None,
            promotedto: attr_owned(&e, "promotedto"),
            deprecatedby: attr_owned(&e, "deprecatedby"),
            obsoletedby: attr_owned(&e, "obsoletedby"),
            provides_commands: vec![],
            provides_types: vec![],
            provides_enums: vec![],
        };
        collect_require_blocks(
            e,
            &mut ext.provides_commands,
            &mut ext.provides_types,
            &mut ext.provides_enums,
            Some(&name),
            &mut reg.enum_extensions,
            &mut reg.aliases,
        );
        reg.extensions_order.push(name.clone());
        reg.extensions.insert(name, ext);
    }
}

fn collect_require_blocks<'a>(
    container: Node,
    commands: &mut Vec<String>,
    types_out: &mut Vec<String>,
    enums_out: &mut Vec<String>,
    origin: Option<&'a str>,
    enum_exts: &mut Vec<EnumExtension>,
    aliases: &mut std::collections::BTreeMap<String, String>,
) {
    for r in children_with_tag(container, "require") {
        // `<require api="vulkansc">` blocks contribute SC-only entries.
        // Skip them so the vulkan-only index doesn't accidentally surface
        // VulkanSC-specific structures (e.g. *_SAFETY_CRITICAL_*).
        if let Some(api) = r.attribute("api") {
            if !api.split(',').any(|s| s.trim() == "vulkan") {
                continue;
            }
        }
        for child_n in r.children().filter(Node::is_element) {
            let Some(name) = attr_owned(&child_n, "name") else { continue };
            // Aliased enumerants (extension-introduced names that point at a
            // core enumerant) live inside <require>/<enum name="X" alias="Y" …>.
            // Capture them into the forward alias map so find_callers /
            // resolve_alias can follow them.
            let alias_attr = attr_owned(&child_n, "alias");
            if let Some(target) = &alias_attr {
                aliases.insert(name.clone(), target.clone());
            }
            match child_n.tag_name().name() {
                "command" => commands.push(name),
                "type" => types_out.push(name),
                "enum" => {
                    enums_out.push(name.clone());
                    if let Some(extends) = attr_owned(&child_n, "extends") {
                        enum_exts.push(EnumExtension {
                            extends,
                            name,
                            value: attr_owned(&child_n, "value"),
                            bitpos: attr_owned(&child_n, "bitpos"),
                            offset: attr(&child_n, "offset").and_then(|s| s.parse::<i32>().ok()),
                            extnumber: attr(&child_n, "extnumber").and_then(|s| s.parse::<i32>().ok()),
                            dir: attr_owned(&child_n, "dir"),
                            alias: alias_attr,
                            comment: attr_owned(&child_n, "comment"),
                            origin: origin.map(str::to_string),
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TINY_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<registry>
  <types>
    <type category="handle" objtypeenum="VK_OBJECT_TYPE_INSTANCE"><type>VK_DEFINE_HANDLE</type>(<name>VkInstance</name>)</type>
    <type category="handle" parent="VkInstance" objtypeenum="VK_OBJECT_TYPE_DEVICE"><type>VK_DEFINE_NON_DISPATCHABLE_HANDLE</type>(<name>VkSurfaceKHR</name>)</type>
    <type category="struct" name="VkExtent2D" returnedonly="true">
      <member><type>uint32_t</type> <name>width</name></member>
      <member><type>uint32_t</type> <name>height</name></member>
    </type>
  </types>
  <commands>
    <command successcodes="VK_SUCCESS" errorcodes="VK_ERROR_OUT_OF_HOST_MEMORY" queues="graphics">
      <proto><type>VkResult</type> <name>vkDoThing</name></proto>
      <param optional="true" externsync="true"><type>VkInstance</type> <name>instance</name></param>
      <param len="null-terminated">const <type>char</type>* <name>pName</name></param>
    </command>
    <command name="vkDoThingKHR" alias="vkDoThing"/>
  </commands>
  <feature api="vulkan" name="VK_VERSION_1_0" number="1.0">
    <require>
      <command name="vkDoThing"/>
      <type name="VkInstance"/>
    </require>
  </feature>
  <extensions>
    <extension name="VK_KHR_surface" number="1" type="instance" supported="vulkan" author="KHR">
      <require>
        <type name="VkSurfaceKHR"/>
      </require>
    </extension>
  </extensions>
</registry>"#;

    #[test]
    fn parses_tiny_registry() {
        let reg = parse_registry(TINY_XML).expect("parse");
        assert_eq!(reg.types.len(), 3);
        // Handles
        let inst = reg.types.get("VkInstance").unwrap();
        assert_eq!(inst.category, "handle");
        assert!(inst.dispatchable);
        let surf = reg.types.get("VkSurfaceKHR").unwrap();
        assert!(!surf.dispatchable);
        assert_eq!(surf.parent.as_deref(), Some("VkInstance"));
        // Struct
        let ext2d = reg.types.get("VkExtent2D").unwrap();
        assert_eq!(ext2d.category, "struct");
        assert!(ext2d.returnedonly);
        assert_eq!(ext2d.members.len(), 2);
        assert_eq!(ext2d.members[0].name, "width");
        assert_eq!(ext2d.members[0].ty, "uint32_t");
        // Commands
        assert_eq!(reg.commands.len(), 2);
        let cmd = reg.commands.get("vkDoThing").unwrap();
        assert_eq!(cmd.return_type, "VkResult");
        assert_eq!(cmd.params.len(), 2);
        assert_eq!(cmd.params[0].name, "instance");
        assert!(cmd.params[0].optional);
        assert_eq!(cmd.params[0].externsync.as_deref(), Some("true"));
        assert_eq!(cmd.success_codes, vec!["VK_SUCCESS"]);
        assert_eq!(cmd.error_codes, vec!["VK_ERROR_OUT_OF_HOST_MEMORY"]);
        assert_eq!(cmd.queues, vec!["graphics"]);
        // Alias
        let alias_cmd = reg.commands.get("vkDoThingKHR").unwrap();
        assert_eq!(alias_cmd.alias.as_deref(), Some("vkDoThing"));
        assert_eq!(reg.aliases.get("vkDoThingKHR"), Some(&"vkDoThing".to_string()));
        // Feature
        let f = reg.features.get("VK_VERSION_1_0").unwrap();
        assert_eq!(f.api, "vulkan");
        assert!(f.provides_commands.contains(&"vkDoThing".to_string()));
        assert!(f.provides_types.contains(&"VkInstance".to_string()));
        // Extension
        let ext = reg.extensions.get("VK_KHR_surface").unwrap();
        assert_eq!(ext.number, Some(1));
        assert_eq!(ext.ty.as_deref(), Some("instance"));
        assert!(ext.provides_types.contains(&"VkSurfaceKHR".to_string()));
    }

    #[test]
    fn const_and_pointer_depth_match_inner_text() {
        let reg = parse_registry(TINY_XML).unwrap();
        let cmd = reg.commands.get("vkDoThing").unwrap();
        let p = &cmd.params[1];
        assert_eq!(p.name, "pName");
        assert_eq!(p.ty, "char");
        assert!(p.is_const);
        assert_eq!(p.pointer_depth, 1);
        assert_eq!(p.len.as_deref(), Some("null-terminated"));
    }
}
