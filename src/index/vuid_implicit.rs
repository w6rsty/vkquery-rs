//! Implicit VUID derivation from `vk.xml` attributes.
//!
//! Reverse-engineering of Vulkan-Docs `scripts/validitygenerator.py` rules.
//! Goal: 100% id parity vs Python's `ValidityOutputGenerator`, ≥95% text
//! parity (some whitespace / phrasing drift is acceptable).
//!
//! Coverage in this pass (≈80% of the 6575 implicit VUIDs at HEAD):
//! - commands: `commandBuffer-recording`, `commandBuffer-cmdpool`,
//!   per-param `-parameter` (handle / pointer / array / enum / bitmask /
//!   struct), per-handle-param `-parent` / `-commonparent`.
//! - structs:  `sType-sType`, `pNext-pNext` (NULL / single / multi extender),
//!   `sType-unique`, per-member `-parameter`, `-arraylength`,
//!   `-requiredbitmask`, `-zerobitmask`.
//!
//! Out of scope this pass (will be iterated): `device-queuecount`,
//! `suspended`, `videocoding`, `sparseimage`, `viewtype`, `externsync` extras.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde_json::{json, Value};

use crate::registry::{Command, Member, Param, Registry, Type};

/// Basic C types that never need a `*-parameter` VUID. Matches Python's
/// `TYPES_KNOWN_ALWAYS_VALID` in `spec_tools/conventions.py`.
const TYPES_ALWAYS_VALID: &[&str] = &[
    "char", "float",
    "int8_t", "uint8_t", "int16_t", "uint16_t",
    "int32_t", "uint32_t", "int64_t", "uint64_t",
    "size_t", "intptr_t", "uintptr_t", "int",
];

/// Categories that ALWAYS require validation. Vulkan-specific override from
/// `vkconventions.py` — excludes `basetype` and `None` (vs. default which
/// includes both).
fn category_requires_validation(cat: &str) -> bool {
    matches!(cat, "handle" | "enum" | "bitmask")
}

fn type_always_valid(ty: &str) -> bool {
    TYPES_ALWAYS_VALID.contains(&ty)
}

// ---- public entry --------------------------------------------------------

/// Derive implicit VUIDs for every command and struct in the registry.
/// `extended_by` maps struct name → list of struct names whose `structextends=`
/// includes it (already computed by `reverse::build_reverse`).
pub fn derive_from_registry(
    reg: &Registry,
    extended_by: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, Value> {
    let mut out: BTreeMap<String, Value> = BTreeMap::new();

    let ctx = Context::new(reg);

    // Pre-compute the set of commands and types that any vulkan-api feature
    // or extension provides. Python's generator only emits validity for the
    // currently-targeted API surface; we mirror that to avoid leaking
    // vulkansc / OS-shim-only commands (OHOS, FUCHSIA, etc. that aren't
    // required by any vulkan feature) into the output.
    let (vulkan_cmds, vulkan_types) = collect_vulkan_surface(reg);

    for (cname, cmd) in &reg.commands {
        if !vulkan_cmds.contains(cname.as_str()) {
            continue;
        }
        let source = if cmd.alias.is_some() {
            resolve_command_alias(reg, cname).unwrap_or(cmd)
        } else {
            cmd
        };
        emit_for_command(cname, source, &ctx, &mut out);
    }

    for (tname, ty) in &reg.types {
        if ty.category != "struct" {
            continue;
        }
        if ty.alias.is_some() {
            continue;
        }
        if !vulkan_types.contains(tname.as_str()) {
            continue;
        }
        emit_for_struct_real(tname, ty, &ctx, extended_by, &vulkan_types, &mut out);
    }

    out
}

/// Build (commands, types) sets covering every entity any vulkan-api feature
/// or extension provides. Mirrors `xml_index::is_vulkan_feature` /
/// `is_vulkan_extension` discrimination.
fn collect_vulkan_surface(reg: &Registry) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut cmds: BTreeSet<String> = BTreeSet::new();
    let mut types: BTreeSet<String> = BTreeSet::new();
    for f in reg.features.values() {
        // Internal sub-features (apitype="internal") are aggregated into the
        // public feature — they're vulkan if their parent is.
        let on_vulkan = f.api.split(',').any(|s| s.trim() == "vulkan");
        if !on_vulkan {
            continue;
        }
        cmds.extend(f.provides_commands.iter().cloned());
        types.extend(f.provides_types.iter().cloned());
    }
    for e in reg.extensions.values() {
        if !e.supported.iter().any(|s| s == "vulkan") {
            continue;
        }
        cmds.extend(e.provides_commands.iter().cloned());
        types.extend(e.provides_types.iter().cloned());
    }
    (cmds, types)
}

// ---- small shared context ------------------------------------------------

struct Context<'a> {
    reg: &'a Registry,
    /// Names of features + extensions enabled for the vulkan API surface.
    /// Used to filter `enum_extensions` so disabled extensions
    /// (`supported="disabled"` or vulkansc-only) don't pretend their
    /// bit values populate a FlagBits enum.
    active_origins: BTreeSet<String>,
    /// Memoization for [`Context::is_struct_always_valid`]. The recursive
    /// member walk is hot — every command/struct parameter triggers it — and
    /// `VkAllocationCallbacks` / `VkExtent2D` / … get hit hundreds of times.
    /// `RefCell` because the visitor signature is `&self`.
    always_valid_cache: RefCell<HashMap<String, bool>>,
}

impl<'a> Context<'a> {
    fn new(reg: &'a Registry) -> Self {
        let mut active_origins: BTreeSet<String> = BTreeSet::new();
        for (name, f) in &reg.features {
            if f.api.split(',').any(|s| s.trim() == "vulkan") {
                active_origins.insert(name.clone());
            }
        }
        for (name, e) in &reg.extensions {
            if e.supported.iter().any(|s| s == "vulkan") {
                active_origins.insert(name.clone());
            }
        }
        Self {
            reg,
            active_origins,
            always_valid_cache: RefCell::new(HashMap::new()),
        }
    }

    fn type_category(&self, name: &str) -> Option<&str> {
        self.reg.types.get(name).map(|t| t.category.as_str())
    }

    fn handle_parent(&self, name: &str) -> Option<String> {
        self.reg.types.get(name).and_then(|t| t.parent.clone())
    }

    /// For a bitmask `VkFooFlags`, return the matching `VkFooFlagBits` enum
    /// name to use in the spec text. Preference order: explicit `bitvalues=`
    /// / `requires=` on the `<type>`, then the string-substitution heuristic
    /// `Flags → FlagBits`. Aliases are resolved through `<type alias=...>`
    /// (so VkPipelineStageFlags2KHR finds VkPipelineStageFlagBits2 via
    /// VkPipelineStageFlags2). Mirrors validitygenerator.py:770-779 plus
    /// `lookupElementInfo` alias chase.
    /// Returns None when the FlagBits enum has no values active under the
    /// vulkan API surface — callers should emit `zerobitmask` instead.
    fn bitmask_requires(&self, name: &str) -> Option<String> {
        let ty = self.reg.types.get(name)?;
        if ty.category != "bitmask" {
            return None;
        }
        // If this is itself an alias bitmask, resolve to the canonical
        // bitmask type. The text we return still uses the *aliased* FlagBits
        // name to match Python's "makeEnumerationName(bitsname)" output —
        // bitsname is the substitution from the original aliased name.
        let canonical = ty
            .alias
            .as_deref()
            .and_then(|t| self.reg.types.get(t))
            .unwrap_or(ty);
        let bits_name_aliased = canonical
            .requires
            .clone()
            .unwrap_or_else(|| name.replace("Flags", "FlagBits"));
        if bits_name_aliased == name {
            return None;
        }
        // Resolve the FlagBits alias chain to find the enum block that
        // actually carries the values.
        let mut bits_name = bits_name_aliased.clone();
        for _ in 0..4 {
            if self.reg.enums.contains_key(&bits_name) {
                break;
            }
            match self
                .reg
                .types
                .get(&bits_name)
                .and_then(|t| t.alias.clone())
            {
                Some(target) => bits_name = target,
                None => break,
            }
        }
        // The `<enums>` block must have at least one enumerant — or a
        // FlagBits-extending injection from a *vulkan-active* feature or
        // extension. Disabled-extension injections (e.g.
        // VK_NV_extension_104 → VkPrivateDataSlotCreateFlagBits) don't
        // count: Python's bitselem.findall('enum[@required="true"]') excludes
        // them via reg.py's `required=` flag.
        let has_values = self
            .reg
            .enums
            .get(&bits_name)
            .map(|e| !e.values.is_empty())
            .unwrap_or(false)
            || self.reg.enum_extensions.iter().any(|ex| {
                ex.extends == bits_name
                    && ex
                        .origin
                        .as_deref()
                        .map(|o| self.active_origins.contains(o))
                        .unwrap_or(false)
            });
        if !has_values {
            return None;
        }
        // Return the *aliased* name so the spec text matches Python's output
        // (e.g. "valid combination of VkPipelineStageFlagBits2KHR values"
        // rather than the canonical "...FlagBits2").
        Some(bits_name_aliased)
    }

    /// Port of `generator.py:isStructAlwaysValid`. Returns true for structs/
    /// unions whose members are all trivially valid (no handles, enums,
    /// bitmasks, pointers, arrays, `sType`/`pNext`, `noautovalidity`, or
    /// nested non-always-valid structs). When true, the non-pointer
    /// non-array `*-parameter` VUID is suppressed at the call site
    /// (Python validitygenerator.py:822-827).
    fn is_struct_always_valid(&self, name: &str) -> bool {
        if type_always_valid(name) {
            return true;
        }
        if let Some(hit) = self.always_valid_cache.borrow().get(name) {
            return *hit;
        }
        // Insert `false` up-front to break cycles in self-referential structs
        // — Python relies on the recursion limit; we just choose a safe answer.
        self.always_valid_cache.borrow_mut().insert(name.to_string(), false);
        let result = self.compute_struct_always_valid(name);
        self.always_valid_cache.borrow_mut().insert(name.to_string(), result);
        result
    }

    fn compute_struct_always_valid(&self, name: &str) -> bool {
        let Some(ty) = self.reg.types.get(name) else {
            return false;
        };
        if category_requires_validation(&ty.category) {
            return false;
        }
        if ty.category != "struct" && ty.category != "union" {
            // basetypes / funcpointers / defines aren't structs — Python
            // returns from the typedict lookup and walks empty members,
            // returning true. Match that.
            return true;
        }
        for m in &ty.members {
            if m.name == "sType" || m.name == "pNext" {
                return false;
            }
            if m.noautovalidity {
                return false;
            }
            if m.ty == "void" || m.ty == "char" {
                return false;
            }
            if m.pointer_depth > 0 || m.len.is_some() {
                return false;
            }
            if type_always_valid(&m.ty) {
                continue;
            }
            let mcat = self.type_category(&m.ty).unwrap_or("");
            if category_requires_validation(mcat) {
                return false;
            }
            if (mcat == "struct" || mcat == "union") && !self.is_struct_always_valid(&m.ty) {
                return false;
            }
        }
        true
    }
}

// ---- commands ------------------------------------------------------------

fn emit_for_command(
    cname: &str,
    cmd: &Command,
    ctx: &Context,
    out: &mut BTreeMap<String, Value>,
) {
    let is_cmd_buf = cname.starts_with("vkCmd");

    // `commandBuffer-recording` for every vkCmd*.
    if is_cmd_buf {
        emit(
            out,
            cname,
            "commandBuffer",
            "recording",
            "commandBuffer must: be in the recording state",
            command_source(cname),
        );
        // `commandBuffer-cmdpool` based on `queues=` attribute.
        let queues = pretty_queue_list(&cmd.queues);
        if !queues.is_empty() {
            let text = format!(
                "The VkCommandPool that commandBuffer was allocated from must: support {queues} operations"
            );
            emit(out, cname, "commandBuffer", "cmdpool", &text, command_source(cname));
        }
    }

    // `device-queuecount` on vkCreate*/vkAllocate* whose first param is the
    // `device` handle. Suppressed by `allownoqueues="true"` on the command
    // (e.g. `vkCreatePipelines`-family which work without any queues).
    // Mirrors validitygenerator.py:1329-1334.
    if (cname.starts_with("vkCreate") || cname.starts_with("vkAllocate"))
        && !cmd.allow_no_queues
        && cmd.params.first().map(|p| p.name.as_str()) == Some("device")
    {
        emit(
            out,
            cname,
            "device",
            "queuecount",
            "The device must: have been created with at least 1 queue",
            command_source(cname),
        );
    }

    // Each param's `-parameter` + `-parent` + `-arraylength` (the last when
    // another param's `len=` references it).
    for p in &cmd.params {
        let cat = ctx.type_category(&p.ty).unwrap_or("");
        emit_param_validity(p, &cmd.params, cname, cat, ctx, out, command_source(cname));

        // `-parent`: emitted for direct handle params AND const-pointer
        // handle arrays (input arrays of handles). Non-const handle pointers
        // are output params — skip. Mirrors validitygenerator.py line 1412.
        if cat == "handle" {
            let is_output_ptr = p.pointer_depth > 0 && !p.is_const;
            if !is_output_ptr {
                emit_handle_parent(p, cmd, cname, ctx, out);
            }
        }
    }
    // Emit `-arraylength` for params that are referenced as the `len=` of some
    // other param.
    emit_param_arraylengths(cmd, ctx, cname, out);
}

/// Find a parameter providing `target_ty` (handle ancestor). Walks up the
/// handle parent chain when no direct match is found — e.g. a command with
/// `VkCommandBuffer` but only `VkDevice` (not `VkCommandPool`) listed will
/// fall back to VkDevice. Matches Python `makeHandleValidityParent` lines
/// 867-881.
fn emit_handle_parent(
    p: &Param,
    cmd: &Command,
    cname: &str,
    ctx: &Context,
    out: &mut BTreeMap<String, Value>,
) {
    let mut current = p.ty.clone();
    let parent_param: Option<&Param> = loop {
        let Some(handle_parent_ty) = ctx.handle_parent(&current) else {
            break None;
        };
        if let Some(found) = cmd.params.iter().find(|q| q.ty == handle_parent_ty) {
            break Some(found);
        }
        current = handle_parent_ty;
    };
    let Some(parent_param) = parent_param else { return };
    let is_optional = p.optional || p.noautovalidity;
    let is_array = p.pointer_depth > 0 && p.len.is_some();
    let subject = if is_array {
        if is_optional {
            format!("Each element of {} that is a valid handle", p.name)
        } else {
            format!("Each element of {}", p.name)
        }
    } else if is_optional {
        format!("If {} is a valid handle, it", p.name)
    } else {
        p.name.clone()
    };
    let text = format!(
        "{subject} must: have been created, allocated, or retrieved from {}",
        parent_param.name
    );
    emit(out, cname, &p.name, "parent", &text, command_source(cname));
}

fn emit_param_validity(
    p: &Param,
    all_params: &[Param],
    entity: &str,
    cat: &str,
    ctx: &Context,
    out: &mut BTreeMap<String, Value>,
    source_file: String,
) {
    if p.noautovalidity {
        return;
    }
    // Skip the "tasks" parameter etc. — those aren't real params.
    let pname = &p.name;
    let ty = &p.ty;
    let optional_first = p.optional; // for pointer / array we care about first level

    // `void*` pointer (single, optional): Python's validitygenerator returns
    // None for `pointercount == 1 and optional is not None` (line 689). The
    // member never gets a -parameter VUID. Mirror that to drop pUserData /
    // similar void-pointer extras.
    if p.pointer_depth == 1 && p.len.is_none() && ty == "void" && p.optional {
        return;
    }

    // Pointer / array path
    if p.pointer_depth > 0 {
        // If len= references something, it's an array.
        if let Some(len) = &p.len {
            // `len` may be "null-terminated", a count param name, or "1".
            if let Some(text) = pointer_array_text(p, len, ty, cat, ctx, all_params) {
                emit(out, entity, pname, "parameter", &text, source_file.clone());
            }
            return;
        }
        // Pointer to single. If returnedonly (output pointer) we omit "valid
        // X structure" — keep as "valid pointer to a TY handle/value" for
        // output. Simplest: distinguish by ctx; structs say "structure",
        // handles say "handle", others say "value".
        let prefix = pointer_prefix(p, "");
        let inner = pointer_single_inner(ty, cat, ctx, p);
        let text = format!("{prefix}{pname} must: be a valid pointer to {inner}");
        emit(out, entity, pname, "parameter", &text, source_file);
        return;
    }

    // Non-pointer path
    match cat {
        "handle" => {
            let prefix = if optional_first {
                format!("If {pname} is not VK_NULL_HANDLE, ")
            } else {
                String::new()
            };
            let text = format!("{prefix}{pname} must: be a valid {ty} handle");
            emit(out, entity, pname, "parameter", &text, source_file);
        }
        "enum" => {
            let text = format!("{pname} must: be a valid {ty} value");
            emit(out, entity, pname, "parameter", &text, source_file);
        }
        "bitmask" => {
            let requires = ctx.bitmask_requires(ty);
            match (requires, optional_first) {
                (Some(flagbits), false) => {
                    let text = format!(
                        "{pname} must: be a valid combination of {flagbits} values"
                    );
                    emit(out, entity, pname, "parameter", &text, source_file.clone());
                    // Required bitmask (no optional) → must not be 0
                    let req = format!("{pname} must: not be 0");
                    emit(out, entity, pname, "requiredbitmask", &req, source_file);
                }
                (Some(flagbits), true) => {
                    let text = format!(
                        "{pname} must: be a valid combination of {flagbits} values"
                    );
                    emit(out, entity, pname, "parameter", &text, source_file);
                }
                (None, _) => {
                    // Empty/placeholder bitmask → `zerobitmask` anchor, not
                    // `parameter`. Matches validitygenerator.py line 783-789.
                    let text = format!("{pname} must: be 0");
                    emit(out, entity, pname, "zerobitmask", &text, source_file);
                }
            }
        }
        "struct" => {
            // Non-pointer, non-array struct param: suppress VUID if the
            // struct is "always valid" (all members are trivially valid).
            // Matches validitygenerator.py:821-823 leaving typetext=None.
            if ctx.is_struct_always_valid(ty) {
                return;
            }
            let text = format!("{pname} must: be a valid {ty} structure");
            emit(out, entity, pname, "parameter", &text, source_file);
        }
        "union" => {
            if ctx.is_struct_always_valid(ty) {
                return;
            }
            let text = format!("{pname} must: be a valid {ty} union");
            emit(out, entity, pname, "parameter", &text, source_file);
        }
        "basetype" if ty == "VkDeviceAddress" => {
            // Python only special-cases `VkDeviceAddress` among basetypes —
            // generic numeric basetypes (VkDeviceSize, VkBool32, VkFlags…)
            // don't get a "value" VUID. See validitygenerator.py:836-844.
            let prefix = if optional_first {
                format!("If {pname} is not 0, ")
            } else {
                String::new()
            };
            let text = format!("{prefix}{pname} must: be a valid {ty} value");
            emit(out, entity, pname, "parameter", &text, source_file);
        }
        "funcpointer" => {
            let prefix = if optional_first {
                format!("If {pname} is not NULL, ")
            } else {
                String::new()
            };
            let text = format!("{prefix}{pname} must: be a valid {ty} value");
            emit(out, entity, pname, "parameter", &text, source_file);
        }
        _ => {}
    }
}

fn pointer_single_inner(ty: &str, cat: &str, _ctx: &Context, p: &Param) -> String {
    // "a valid X handle" / "a valid X structure" / "a valid X value" / "a TY handle"
    // For output-only pointers (e.g. `pImage`), Python says "a VkImage handle"
    // (no "valid") — distinguished by being non-const pointer.
    let is_output = !p.is_const;
    if cat == "handle" {
        if is_output {
            format!("a {ty} handle")
        } else {
            format!("a valid {ty} handle")
        }
    } else if cat == "struct" || cat == "union" {
        format!("a valid {ty} structure")
    } else {
        // basetype / int / void* etc.
        format!("a valid {ty} value")
    }
}

fn pointer_prefix(p: &Param, _ty: &str) -> String {
    if p.optional {
        format!("If {} is not NULL, ", p.name)
    } else {
        String::new()
    }
}

fn pointer_array_text(
    p: &Param,
    len: &str,
    ty: &str,
    cat: &str,
    ctx: &Context,
    all_params: &[Param],
) -> Option<String> {
    let pname = &p.name;
    // "null-terminated" arrays render as "valid null-terminated UTF-8 string"
    if len == "null-terminated" {
        let prefix = if p.optional {
            format!("If {pname} is not NULL, ")
        } else {
            String::new()
        };
        return Some(format!(
            "{prefix}{pname} must: be a valid pointer to a null-terminated UTF-8 string"
        ));
    }
    // The len may chain, e.g. "rasterizationSamples,1" — Python takes the
    // first count name as the "size" and ignores the rest. Trim suffixes.
    let primary_len = len.split(',').next().unwrap_or(len);
    // Determine the inner-element wording.
    let element = match cat {
        "handle" => format!("{ty} handles"),
        "struct" | "union" => format!("valid {ty} structures"),
        "enum" => format!("valid {ty} values"),
        "bitmask" => format!("valid {ty} values"),
        _ => format!("{ty} values"),
    };
    // Check if the array's length comes from another param/member that's
    // optional itself (and is non-zero gated). Build prefix conditions:
    let mut conditions: Vec<String> = Vec::new();
    if !primary_len.is_empty() && !is_numeric(primary_len) {
        // Find the count param.
        if let Some(count_param) = all_params.iter().find(|q| q.name == primary_len) {
            if count_param.optional {
                conditions.push(format!("{} is not 0", count_param.name));
            }
        }
    }
    if p.optional {
        conditions.push(format!("{pname} is not NULL"));
    }
    let prefix = if conditions.is_empty() {
        String::new()
    } else {
        format!("If {}, ", conditions.join(", and "))
    };
    let valid_word = if cat == "handle" { "" } else { "valid " };
    // Compose: "X must: be a valid pointer to an array of LEN valid TY values/structures/handles"
    let _ = (valid_word, ctx);
    Some(format!(
        "{prefix}{pname} must: be a valid pointer to an array of {primary_len} {element}"
    ))
}

fn is_numeric(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

fn emit_param_arraylengths(
    cmd: &Command,
    _ctx: &Context,
    cname: &str,
    out: &mut BTreeMap<String, Value>,
) {
    // A param P is an "arraylength" param when it appears in any other
    // param's `len=` attribute AND P itself is not marked `optional="true"`.
    // (If P is optional, 0 is a legal value, so no "> 0" VUID is generated.)
    let mut counters: BTreeSet<&str> = BTreeSet::new();
    for p in &cmd.params {
        if let Some(len) = &p.len {
            for chunk in len.split(',') {
                let chunk = chunk.trim();
                if chunk == "null-terminated" || chunk.is_empty() || is_numeric(chunk) {
                    continue;
                }
                counters.insert(chunk);
            }
        }
    }
    for p in &cmd.params {
        if counters.contains(p.name.as_str()) && !p.optional {
            let text = format!("{} must: be greater than 0", p.name);
            emit(out, cname, &p.name, "arraylength", &text, command_source(cname));
        }
    }
}

// ---- structs -------------------------------------------------------------

fn resolve_command_alias<'a>(reg: &'a Registry, name: &str) -> Option<&'a Command> {
    let mut cur = name.to_string();
    let mut seen = std::collections::HashSet::new();
    while let Some(c) = reg.commands.get(&cur) {
        if c.alias.is_none() {
            return Some(c);
        }
        let target = c.alias.clone().unwrap();
        if !seen.insert(cur.clone()) {
            return Some(c);
        }
        cur = target;
    }
    None
}

fn emit_for_struct_real(
    sname: &str,
    ty: &Type,
    ctx: &Context,
    extended_by: &BTreeMap<String, Vec<String>>,
    vulkan_types: &BTreeSet<String>,
    out: &mut BTreeMap<String, Value>,
) {
    let source = struct_source(sname);
    // Python's `is_structure_type_member` / `is_nextpointer_member` require
    // the member's *type* to match — not just the name. Base structs like
    // VkBaseInStructure have `pNext: VkBaseInStructure*`, so Python treats
    // pNext as a regular typed pointer, not the chain-walk member.
    let mut has_pnext_chain = false;  // true only if pNext is `void*`
    let mut has_stype = false;

    // sType + pNext detection. Members marked `noautovalidity="true"` opt
    // out of automatic VUID generation (Python's `addSharedStructMemberValidity`
    // line 1121).
    for m in &ty.members {
        if m.name == "sType" && !m.noautovalidity && m.ty == "VkStructureType" {
            has_stype = true;
            // Python's makeStructureTypeValidity emits even with empty values
            // — `''.split(',')` returns `['']`, which truthy-passes the
            // values branch and produces a degenerate "sType must: be ename:"
            // (the base-struct case). Match that to keep id parity.
            let val_first = m
                .values
                .as_deref()
                .and_then(|v| v.split(',').next())
                .map(str::trim)
                .unwrap_or("");
            let text = if val_first.is_empty() {
                "sType must: be ename:".to_string()
            } else {
                format!("sType must: be {val_first}")
            };
            emit(out, sname, "sType", "sType", &text, source.clone());
        }
        if m.name == "pNext" && !m.noautovalidity && m.ty == "void" {
            has_pnext_chain = true;
        }
    }

    // pNext-pNext is ONLY emitted when this struct doesn't extend any other
    // struct AND pNext is the canonical `void*` chain member. Extending
    // structs share the chain-root's pNext rule.
    // See validitygenerator.py addSharedStructMemberValidity / line 1129.
    let is_extending = !ty.structextends.is_empty();
    if has_pnext_chain && !is_extending {
        let extenders = extended_by.get(sname).cloned().unwrap_or_default();
        // Filter out (a) aliases and (b) types not part of the vulkan API
        // surface — VulkanSC-only extenders like VkApplicationParametersEXT
        // should not appear in the vulkan-only validity output.
        let extenders_filtered: Vec<&String> = extenders
            .iter()
            .filter(|name| {
                ctx.reg
                    .types
                    .get(name.as_str())
                    .map(|t| t.alias.is_none())
                    .unwrap_or(true)
                    && vulkan_types.contains(name.as_str())
            })
            .collect();
        let text = match extenders_filtered.len() {
            0 => "pNext must: be NULL".to_string(),
            1 => format!(
                "pNext must: be NULL or a pointer to a valid instance of {}",
                extenders_filtered[0]
            ),
            _ => {
                let mut sorted: Vec<&str> = extenders_filtered.iter().map(|s| s.as_str()).collect();
                sorted.sort();
                let prose = prose_list_or(&sorted);
                format!(
                    "Each pNext member of any structure (including this one) in the pNext chain must: be either NULL or a pointer to a valid instance of {prose}"
                )
            }
        };
        emit(out, sname, "pNext", "pNext", &text, source.clone());

        // sType-unique: emit when struct has pNext AND any extender exists.
        if !extenders_filtered.is_empty() && has_stype {
            let text = "The sType value of each structure in the pNext chain must: be unique";
            emit(out, sname, "sType", "unique", text, source.clone());
        }
    }

    // returnedonly structs: ONLY sType + pNext, no member-by-member validity.
    if ty.returnedonly {
        return;
    }

    // Members validity. Skip sType/pNext ONLY when they match the canonical
    // pattern that `addSharedStructMemberValidity` already handled
    // (sType: VkStructureType, pNext: void*). Otherwise they fall through to
    // regular member validation — the base structs use a typed pNext that
    // needs `pNext-parameter` not `pNext-pNext`.
    for m in &ty.members {
        let handled_as_shared = (m.name == "sType" && m.ty == "VkStructureType")
            || (m.name == "pNext" && m.ty == "void");
        if handled_as_shared {
            continue;
        }
        // Tagged-union member (`selector="discriminator"`): suppress the
        // direct `-parameter` VUID for the union and emit per-branch VUIDs
        // gated on the selector's value. See validitygenerator.py:1224-1239.
        // Selectors fire even when the parent member is `noautovalidity` —
        // the noautovalidity only suppresses the union-as-a-whole VUID, not
        // the per-branch validities. Python checks noautovalidity on the
        // individual union members instead.
        if let Some(selector) = &m.selector {
            emit_union_selector_branches(m, selector, sname, ctx, out, source.clone());
            continue;
        }
        if m.noautovalidity {
            continue;
        }
        emit_member_validity(m, &ty.members, sname, ctx, out, source.clone());
    }

    // arraylength for member counts
    emit_member_arraylengths(&ty.members, sname, out);
}

/// Emit per-branch VUIDs for a tagged-union member. The parent struct
/// member `parent` has type `union_type` (a union); the parent's
/// `selector="discriminator"` member determines which branch is active.
/// Each union member's `selection="VALUE,..."` lists the discriminator
/// values that activate that branch.
fn emit_union_selector_branches(
    parent: &Member,
    selector_name: &str,
    entity: &str,
    ctx: &Context,
    out: &mut BTreeMap<String, Value>,
    source_file: String,
) {
    let Some(union_type) = ctx.reg.types.get(&parent.ty) else { return };
    if union_type.category != "union" {
        return;
    }
    for um in &union_type.members {
        if um.noautovalidity {
            continue;
        }
        let Some(selection) = &um.selection else { continue };
        let values: Vec<&str> = selection.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
        if values.is_empty() {
            continue;
        }
        let prose_values = prose_list_or(&values);
        let prefix = format!("If {selector_name} is {prose_values}, ");
        let umname = &um.name;
        let pname = &parent.name;

        let inner_text = if um.pointer_depth > 0 {
            // Pointer-to-single or array of unioned type.
            let cat = ctx.type_category(&um.ty).unwrap_or("");
            if let Some(len) = &um.len {
                // Pointer with len → array. Use the simple count form.
                let count = len.split(',').next().unwrap_or(len);
                let element = match cat {
                    "handle" => format!("{} handles", um.ty),
                    "struct" | "union" => format!("valid {} structures", um.ty),
                    "enum" | "bitmask" => format!("valid {} values", um.ty),
                    _ => format!("{} values", um.ty),
                };
                let opt_prefix = if um.optional {
                    format!("and if {umname} is not NULL, ")
                } else {
                    String::new()
                };
                if is_numeric(count) || count.is_empty() {
                    format!(
                        "{opt_prefix}the {umname} member of {pname} must: be a valid pointer to {element}"
                    )
                } else {
                    format!(
                        "{opt_prefix}the {umname} member of {pname} must: be a valid pointer to an array of {count} {element}"
                    )
                }
            } else {
                let opt_prefix = if um.optional {
                    format!("and if {umname} is not NULL, ")
                } else {
                    String::new()
                };
                let inner = match cat {
                    "handle" => format!("a valid {} handle", um.ty),
                    "struct" | "union" => format!("a valid {} structure", um.ty),
                    _ => format!("a valid {} value", um.ty),
                };
                format!(
                    "{opt_prefix}the {umname} member of {pname} must: be a valid pointer to {inner}"
                )
            }
        } else {
            // Non-pointer union member. Python emits only when the type
            // *needs* validation: basic types (uint32_t etc.) and
            // always-valid structs/unions produce no entry. Mirrors
            // createValidationLineForParameter's `typecategory is None`
            // and struct/union arms gated on `needs_recursive_validity`.
            let cat = ctx.type_category(&um.ty).unwrap_or("");
            if type_always_valid(&um.ty) {
                continue;
            }
            match cat {
                "handle" => format!("the {umname} member of {pname} must: be a valid {} handle", um.ty),
                "enum" => format!("the {umname} member of {pname} must: be a valid {} value", um.ty),
                "struct" => {
                    if ctx.is_struct_always_valid(&um.ty) {
                        continue;
                    }
                    format!("the {umname} member of {pname} must: be a valid {} structure", um.ty)
                }
                "union" => {
                    if ctx.is_struct_always_valid(&um.ty) {
                        continue;
                    }
                    format!("the {umname} member of {pname} must: be a valid {} union", um.ty)
                }
                "basetype" if um.ty == "VkDeviceAddress" => {
                    // Python special-cases VkDeviceAddress among basetypes
                    // (validitygenerator.py:836-837) — generic basetypes
                    // (VkDeviceSize, VkBool32) don't emit a "valid value"
                    // VUID, but VkDeviceAddress does.
                    format!("the {umname} member of {pname} must: be a valid {} value", um.ty)
                }
                _ => continue,
            }
        };

        let text = format!("{prefix}{inner_text}");
        emit(out, entity, umname, "parameter", &text, source_file.clone());
    }
}

fn emit_member_validity(
    m: &Member,
    all_members: &[Member],
    entity: &str,
    ctx: &Context,
    out: &mut BTreeMap<String, Value>,
    source_file: String,
) {
    let cat = ctx.type_category(&m.ty).unwrap_or("");
    let mname = &m.name;

    if m.pointer_depth > 0 {
        if let Some(len) = &m.len {
            if let Some(text) = member_pointer_array_text(m, len, cat, all_members) {
                emit(out, entity, mname, "parameter", &text, source_file);
            }
            return;
        }
        // `void*` member (single, optional) — Python suppresses (no -parameter
        // VUID) since the contents are opaque user data. Drops pUserData etc.
        if m.pointer_depth == 1 && m.ty == "void" && m.optional {
            return;
        }
        let prefix = if m.optional {
            format!("If {mname} is not NULL, ")
        } else {
            String::new()
        };
        let inner = match cat {
            "handle" => format!("a valid {} handle", m.ty),
            "struct" | "union" => format!("a valid {} structure", m.ty),
            _ => format!("a valid {} value", m.ty),
        };
        let text = format!("{prefix}{mname} must: be a valid pointer to {inner}");
        emit(out, entity, mname, "parameter", &text, source_file);
        return;
    }

    match cat {
        "handle" => {
            let prefix = if m.optional {
                format!("If {mname} is not VK_NULL_HANDLE, ")
            } else {
                String::new()
            };
            let text = format!("{prefix}{mname} must: be a valid {} handle", m.ty);
            emit(out, entity, mname, "parameter", &text, source_file);
        }
        "enum" => {
            let text = format!("{mname} must: be a valid {} value", m.ty);
            emit(out, entity, mname, "parameter", &text, source_file);
        }
        "bitmask" => {
            let requires = ctx.bitmask_requires(&m.ty);
            match (requires, m.optional) {
                (Some(flagbits), false) => {
                    let text = format!(
                        "{mname} must: be a valid combination of {flagbits} values"
                    );
                    emit(out, entity, mname, "parameter", &text, source_file.clone());
                    let req = format!("{mname} must: not be 0");
                    emit(out, entity, mname, "requiredbitmask", &req, source_file);
                }
                (Some(flagbits), true) => {
                    let text = format!(
                        "{mname} must: be a valid combination of {flagbits} values"
                    );
                    emit(out, entity, mname, "parameter", &text, source_file);
                }
                (None, _) => {
                    let text = format!("{mname} must: be 0");
                    emit(out, entity, mname, "zerobitmask", &text, source_file);
                }
            }
        }
        "struct" => {
            if ctx.is_struct_always_valid(&m.ty) {
                return;
            }
            let text = format!("{mname} must: be a valid {} structure", m.ty);
            emit(out, entity, mname, "parameter", &text, source_file);
        }
        "union" => {
            if ctx.is_struct_always_valid(&m.ty) {
                return;
            }
            let text = format!("{mname} must: be a valid {} union", m.ty);
            emit(out, entity, mname, "parameter", &text, source_file);
        }
        "basetype" if m.ty == "VkDeviceAddress" => {
            let prefix = if m.optional {
                format!("If {mname} is not 0, ")
            } else {
                String::new()
            };
            let text = format!("{prefix}{mname} must: be a valid {} value", m.ty);
            emit(out, entity, mname, "parameter", &text, source_file);
        }
        "funcpointer" => {
            let prefix = if m.optional {
                format!("If {mname} is not NULL, ")
            } else {
                String::new()
            };
            let text = format!("{prefix}{mname} must: be a valid {} value", m.ty);
            emit(out, entity, mname, "parameter", &text, source_file);
        }
        _ => {}
    }
}

fn member_pointer_array_text(m: &Member, len: &str, cat: &str, all_members: &[Member]) -> Option<String> {
    let mname = &m.name;
    if len == "null-terminated" {
        let prefix = if m.optional {
            format!("If {mname} is not NULL, ")
        } else {
            String::new()
        };
        return Some(format!(
            "{prefix}{mname} must: be a valid pointer to a null-terminated UTF-8 string"
        ));
    }
    let primary_len = len.split(',').next().unwrap_or(len);
    let element = match cat {
        "handle" => format!("{} handles", m.ty),
        "struct" | "union" => format!("valid {} structures", m.ty),
        "enum" | "bitmask" => format!("valid {} values", m.ty),
        _ => format!("{} values", m.ty),
    };
    let mut conditions: Vec<String> = Vec::new();
    if !primary_len.is_empty() && !is_numeric(primary_len) {
        if let Some(count) = all_members.iter().find(|q| q.name == primary_len) {
            if count.optional {
                conditions.push(format!("{} is not 0", count.name));
            }
        }
    }
    if m.optional {
        conditions.push(format!("{mname} is not NULL"));
    }
    let prefix = if conditions.is_empty() {
        String::new()
    } else {
        format!("If {}, ", conditions.join(", and "))
    };
    Some(format!(
        "{prefix}{mname} must: be a valid pointer to an array of {primary_len} {element}"
    ))
}

fn emit_member_arraylengths(members: &[Member], sname: &str, out: &mut BTreeMap<String, Value>) {
    let mut counters: BTreeSet<&str> = BTreeSet::new();
    for m in members {
        if let Some(len) = &m.len {
            for chunk in len.split(',') {
                let chunk = chunk.trim();
                if chunk == "null-terminated" || chunk.is_empty() || is_numeric(chunk) {
                    continue;
                }
                counters.insert(chunk);
            }
        }
    }
    for m in members {
        if counters.contains(m.name.as_str()) && !m.optional {
            let text = format!("{} must: be greater than 0", m.name);
            emit(out, sname, &m.name, "arraylength", &text, struct_source(sname));
        }
    }
}

// ---- helpers -------------------------------------------------------------

fn command_source(cname: &str) -> String {
    format!("generated/validity/protos/{cname}.adoc")
}

fn struct_source(sname: &str) -> String {
    format!("generated/validity/structs/{sname}.adoc")
}

fn emit(
    out: &mut BTreeMap<String, Value>,
    entity: &str,
    param: &str,
    category: &str,
    text: &str,
    source_file: String,
) {
    let id = format!("VUID-{entity}-{param}-{category}");
    if out.contains_key(&id) {
        return;
    }
    out.insert(
        id.clone(),
        json!({
            "entity": entity,
            "guard_extensions": Vec::<String>::new(),
            "id": id,
            "kind": "implicit",
            "param": param,
            "source_file": source_file,
            "text": text,
        }),
    );
}

/// Convert a `queues=` Vec to "X" / "X, or Y" / "X, Y, or Z" prose.
/// Mirrors Python's `getPrettyQueueList` → `getQueueList` which **sorts**
/// the queue list, then `makeProseList(..., fmt=OR, comma_for_two_elts=True)`.
fn pretty_queue_list(queues: &[String]) -> String {
    let mut clean: Vec<String> = queues
        .iter()
        .flat_map(|q| q.split(',').map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect();
    clean.sort();
    clean.dedup();
    match clean.len() {
        0 => String::new(),
        1 => clean[0].clone(),
        2 => format!("{}, or {}", clean[0], clean[1]),
        _ => {
            let last = clean.pop().unwrap();
            format!("{}, or {}", clean.join(", "), last)
        }
    }
}

/// Convert a slice of strings to a prose `or`-list, comma-for-two.
fn prose_list_or(items: &[&str]) -> String {
    match items.len() {
        0 => String::new(),
        1 => items[0].to_string(),
        2 => format!("{} or {}", items[0], items[1]),
        _ => {
            let last = items[items.len() - 1];
            let head = &items[..items.len() - 1];
            format!("{}, or {}", head.join(", "), last)
        }
    }
}

#[allow(dead_code)]
fn _silence_warnings(_: BTreeSet<()>) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_queue_list_renders_or_prose() {
        assert_eq!(pretty_queue_list(&[]), "");
        assert_eq!(
            pretty_queue_list(&["VK_QUEUE_GRAPHICS_BIT".into()]),
            "VK_QUEUE_GRAPHICS_BIT"
        );
        assert_eq!(
            pretty_queue_list(&[
                "VK_QUEUE_COMPUTE_BIT".into(),
                "VK_QUEUE_GRAPHICS_BIT".into()
            ]),
            "VK_QUEUE_COMPUTE_BIT, or VK_QUEUE_GRAPHICS_BIT"
        );
        assert_eq!(
            pretty_queue_list(&[
                "VK_QUEUE_COMPUTE_BIT".into(),
                "VK_QUEUE_GRAPHICS_BIT".into(),
                "VK_QUEUE_TRANSFER_BIT".into()
            ]),
            "VK_QUEUE_COMPUTE_BIT, VK_QUEUE_GRAPHICS_BIT, or VK_QUEUE_TRANSFER_BIT"
        );
    }

    #[test]
    fn prose_list_or_two_uses_no_comma() {
        assert_eq!(prose_list_or(&["A", "B"]), "A or B");
        assert_eq!(prose_list_or(&["A", "B", "C"]), "A, B, or C");
    }
}
