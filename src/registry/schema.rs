//! Strongly-typed model of `vk.xml`. Populated by `parse.rs`; consumed by
//! `index/*`.
//!
//! The shape here is intentionally close to what reg.py produces (types,
//! commands, extensions, features, aliases), so port logic from
//! `vkquery.index.xml_index` translates 1:1.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub struct Registry {
    pub types: BTreeMap<String, Type>,
    pub commands: BTreeMap<String, Command>,
    pub extensions: BTreeMap<String, Extension>,
    pub features: BTreeMap<String, Feature>,
    /// XML declaration order for `<feature>` elements. Needed because
    /// `feature_origin` attribution prefers the first feature that lists an
    /// entity, matching Python's `reg.apidict` iteration.
    pub features_order: Vec<String>,
    /// XML declaration order for `<extension>` elements. Same reason as
    /// `features_order` — alphabetical iteration would attribute entities to
    /// the wrong extension.
    pub extensions_order: Vec<String>,
    /// Forward alias map: `new_name -> old_name`. Walk transitively at query
    /// time.
    pub aliases: BTreeMap<String, String>,
    /// `<enums>` blocks indexed by `name`.
    pub enums: BTreeMap<String, Enums>,
    /// Loose `<enum>` extensions injected by features/extensions, indexed by
    /// the `extends=` attribute. The same enum can be extended many times.
    pub enum_extensions: Vec<EnumExtension>,
}

/// A `<type>` element. We keep the raw category since downstream code branches
/// on it ("struct" / "union" / "handle" / "enum" / "bitmask" / "funcpointer"
/// / "include" / "define" / "basetype" / "" (unset)).
#[derive(Debug, Clone, Default)]
pub struct Type {
    pub name: String,
    pub category: String,
    pub parent: Option<String>,
    pub objtypeenum: Option<String>,
    pub returnedonly: bool,
    pub structextends: Vec<String>,
    pub alias: Option<String>,
    /// For structs/unions only.
    pub members: Vec<Member>,
    /// For handles only — true if `VK_DEFINE_HANDLE`, false if `VK_DEFINE_NON_DISPATCHABLE_HANDLE`.
    pub dispatchable: bool,
    /// For funcpointers, basetypes, defines — the raw declaration text.
    pub raw: String,
    /// `requires=` attribute (used by externally-included headers like wayland).
    pub requires: Option<String>,
    /// Source file this type was declared in (always `xml/vk.xml`).
    pub source_xml: String,
}

#[derive(Debug, Clone, Default)]
pub struct Member {
    pub name: String,
    pub ty: String,
    pub optional: bool,
    pub len: Option<String>,
    pub externsync: Option<String>,
    pub noautovalidity: bool,
    pub values: Option<String>,
    /// Parent struct's `selector="other_member"` attribute — names a sibling
    /// discriminator member when this member is a tagged union. Per-branch
    /// VUIDs are gated on the selector's value.
    pub selector: Option<String>,
    /// Union member's `selection="VK_ENUM_VALUE,..."` attribute — lists the
    /// discriminator value(s) for which this branch is active. Comma-separated
    /// when one branch covers multiple values (rare).
    pub selection: Option<String>,
    pub is_const: bool,
    pub pointer_depth: i32,
    pub raw_decl: String,
}

#[derive(Debug, Clone, Default)]
pub struct Command {
    pub name: String,
    pub return_type: String,
    pub params: Vec<Param>,
    pub success_codes: Vec<String>,
    pub error_codes: Vec<String>,
    pub queues: Vec<String>,
    pub renderpass: Option<String>,
    pub cmdbufferlevel: Vec<String>,
    pub tasks: Vec<String>,
    /// `allownoqueues="true"` on `<command>` suppresses the
    /// `device-queuecount` implicit VUID on `vkCreate*`/`vkAllocate*` commands
    /// whose first param is `device` (vkCreatePipelines etc. set this).
    pub allow_no_queues: bool,
    pub alias: Option<String>,
    pub raw_decl: String,
}

#[derive(Debug, Clone, Default)]
pub struct Param {
    pub name: String,
    pub ty: String,
    pub optional: bool,
    pub len: Option<String>,
    pub externsync: Option<String>,
    pub noautovalidity: bool,
    pub is_const: bool,
    pub pointer_depth: i32,
    pub raw_decl: String,
}

#[derive(Debug, Clone, Default)]
pub struct Extension {
    pub name: String,
    pub number: Option<i32>,
    pub ty: Option<String>, // "instance" | "device" | None
    pub author: Option<String>,
    pub contact: Option<String>,
    pub supported: Vec<String>,
    pub depends: Option<String>,
    pub requires_extensions: Vec<String>,
    pub requires_core: Option<String>,
    pub promotedto: Option<String>,
    pub deprecatedby: Option<String>,
    pub obsoletedby: Option<String>,
    pub provides_commands: Vec<String>,
    pub provides_types: Vec<String>,
    pub provides_enums: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Feature {
    pub name: String, // e.g. "VK_VERSION_1_4"
    pub number: String, // e.g. "1.4"
    pub api: String, // "vulkan" | "vulkansc" | "vulkan,vulkansc"
    /// `apitype="internal"` on modern vk.xml marks compositional sub-features
    /// like `VK_BASE_VERSION_1_0`, `VK_GRAPHICS_VERSION_1_0` that aggregate
    /// up into the public `VK_VERSION_1_0`. We filter these out at index
    /// time so user-facing JSONs only carry the public name.
    pub apitype: Option<String>,
    pub depends: Option<String>,
    pub provides_commands: Vec<String>,
    pub provides_types: Vec<String>,
    pub provides_enums: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Enums {
    pub name: String,
    /// "enum" | "bitmask" | "constants" | ""
    pub kind: String,
    pub bitwidth: i32,
    pub values: Vec<EnumValue>,
}

#[derive(Debug, Clone, Default)]
pub struct EnumValue {
    pub name: String,
    pub value: Option<String>,
    /// Stored as a string to match Python's JSON shape (`e.get("bitpos")`
    /// returns the raw XML attribute text, no integer cast).
    pub bitpos: Option<String>,
    pub alias: Option<String>,
    pub comment: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct EnumExtension {
    pub extends: String, // the enum being extended
    pub name: String,
    pub value: Option<String>,
    pub bitpos: Option<String>,
    pub offset: Option<i32>,
    pub extnumber: Option<i32>,
    pub dir: Option<String>, // "+" or "-"
    pub alias: Option<String>,
    pub comment: Option<String>,
    /// Which feature/extension introduced this value (informational).
    pub origin: Option<String>,
}
