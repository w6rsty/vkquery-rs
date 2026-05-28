//! Public data types returned by the query API.
//!
//! Field names and shapes mirror the Python `vkquery.types` module so that
//! both implementations produce byte-stable JSON when `serde_json` is called
//! with `sort_keys=True` equivalent (we use `serde_json::to_string_pretty` +
//! field order = struct order; JSON object key ordering matches Python's
//! `sort_keys=True` because we order our struct fields alphabetically here).

use serde::{Deserialize, Serialize};

/// Parameter on a command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Param {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub len: Option<String>,
    #[serde(default)]
    pub externsync: Option<String>,
    #[serde(default)]
    pub noautovalidity: bool,
    #[serde(default)]
    pub r#const: bool,
    #[serde(default)]
    pub pointer_depth: i32,
}

/// Member of a struct.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Member {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub len: Option<String>,
    #[serde(default)]
    pub externsync: Option<String>,
    #[serde(default)]
    pub noautovalidity: bool,
    #[serde(default)]
    pub values: Option<String>,
    #[serde(default)]
    pub r#const: bool,
    #[serde(default)]
    pub pointer_depth: i32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VuidKind {
    Explicit,
    Implicit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Vuid {
    pub id: String,
    pub entity: String,
    pub param: String,
    pub kind: VuidKind,
    pub text: String,
    #[serde(default)]
    pub guard_extensions: Vec<String>,
    #[serde(default)]
    pub source_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct FunctionInfo {
    pub name: String,
    pub return_type: String,
    pub params: Vec<Param>,
    #[serde(default)]
    pub success_codes: Vec<String>,
    #[serde(default)]
    pub error_codes: Vec<String>,
    #[serde(default)]
    pub queues: Vec<String>,
    #[serde(default)]
    pub renderpass: Option<String>,
    #[serde(default)]
    pub cmdbufferlevel: Vec<String>,
    #[serde(default)]
    pub tasks: Vec<String>,
    #[serde(default)]
    pub feature_origin: Option<String>,
    #[serde(default)]
    pub available_in: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub aliased_from: Option<String>,
    #[serde(default)]
    pub vuids: Vec<Vuid>,
    #[serde(default)]
    pub prose_summary: Option<String>,
    #[serde(default)]
    pub source_file: Option<String>,
    #[serde(default)]
    pub section_anchor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct StructInfo {
    pub name: String,
    /// "struct" | "union"
    pub category: String,
    #[serde(default)]
    pub returnedonly: bool,
    #[serde(default)]
    pub structextends: Vec<String>,
    #[serde(default)]
    pub extended_by: Vec<String>,
    #[serde(default)]
    pub members: Vec<Member>,
    #[serde(default)]
    pub feature_origin: Option<String>,
    #[serde(default)]
    pub available_in: Vec<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub aliased_from: Option<String>,
    #[serde(default)]
    pub vuids: Vec<Vuid>,
    #[serde(default)]
    pub prose_summary: Option<String>,
    #[serde(default)]
    pub source_file: Option<String>,
    #[serde(default)]
    pub section_anchor: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExtensionType {
    Instance,
    Device,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExtensionStatus {
    Active,
    Promoted,
    Deprecated,
    Obsoleted,
}

impl Default for ExtensionStatus {
    fn default() -> Self {
        Self::Active
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionInfo {
    pub name: String,
    pub number: Option<i32>,
    #[serde(rename = "type")]
    pub ty: Option<ExtensionType>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub contact: Option<String>,
    #[serde(default)]
    pub supported: Vec<String>,
    #[serde(default)]
    pub depends: Option<String>,
    #[serde(default)]
    pub requires_extensions: Vec<String>,
    #[serde(default)]
    pub requires_core: Option<String>,
    #[serde(default)]
    pub promotedto: Option<String>,
    #[serde(default)]
    pub deprecatedby: Option<String>,
    #[serde(default)]
    pub obsoletedby: Option<String>,
    #[serde(default)]
    pub status: ExtensionStatus,
    #[serde(default)]
    pub provides_commands: Vec<String>,
    #[serde(default)]
    pub provides_types: Vec<String>,
    #[serde(default)]
    pub provides_enums: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandleInfo {
    pub name: String,
    pub parent: Option<String>,
    pub dispatchable: bool,
    #[serde(default)]
    pub objtypeenum: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnumValue {
    pub name: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub bitpos: Option<i32>,
    #[serde(default)]
    pub alias: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnumInfo {
    pub name: String,
    /// "enum" | "bitmask" | "constants"
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub values: Vec<serde_json::Value>,
    #[serde(default)]
    pub extends: Option<String>,
    #[serde(default = "default_bitwidth")]
    pub bitwidth: i32,
    #[serde(default)]
    pub aliases: Vec<String>,
}

fn default_bitwidth() -> i32 {
    32
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CallersResult {
    pub type_name: String,
    pub canonical_name: String,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub structs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DepGraph {
    pub function: String,
    #[serde(default)]
    pub parent_handle_chain: Vec<String>,
    #[serde(default)]
    pub required_features: Vec<String>,
    #[serde(default)]
    pub required_extensions: Vec<String>,
    #[serde(default)]
    pub transitive_extensions: Vec<String>,
    #[serde(default)]
    pub pnext_chain: std::collections::BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub externsync_params: Vec<String>,
    #[serde(default)]
    pub queues: Vec<String>,
    #[serde(default)]
    pub renderpass: Option<String>,
    #[serde(default)]
    pub cmdbufferlevel: Vec<String>,
    #[serde(default)]
    pub error_codes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VuidInfo {
    pub id: String,
    pub entity: String,
    pub param: String,
    pub kind: VuidKind,
    pub text: String,
    #[serde(default)]
    pub guard_extensions: Vec<String>,
    #[serde(default)]
    pub source_file: Option<String>,
    #[serde(default)]
    pub available_in: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiffEntry {
    pub name: String,
    #[serde(default)]
    pub fields: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiffBucket {
    #[serde(default)]
    pub added: Vec<String>,
    #[serde(default)]
    pub removed: Vec<String>,
    #[serde(default)]
    pub changed: Vec<DiffEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiffReport {
    pub from_tag: String,
    pub to_tag: String,
    #[serde(default)]
    pub functions: DiffBucket,
    #[serde(default)]
    pub structs: DiffBucket,
    #[serde(default)]
    pub enums: DiffBucket,
    #[serde(default)]
    pub handles: DiffBucket,
    #[serde(default)]
    pub extensions: DiffBucket,
    #[serde(default)]
    pub features: DiffBucket,
    #[serde(default)]
    pub promoted: Vec<serde_json::Value>,
    #[serde(default)]
    pub vuids: DiffBucket,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SearchHitKind {
    Section,
    Vuid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub score: f32,
    pub kind: SearchHitKind,
    pub source_file: Option<String>,
    pub section_anchor: Option<String>,
    pub entity_hint: Option<String>,
    pub snippet: String,
}
