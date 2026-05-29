//! MCP stdio server exposing the 8 query tools.
//!
//! Tool names and parameter shapes mirror the Python `vkquery.mcp_server`
//! exactly. Returns JSON-serialized payloads wrapped as `Content::text`.

use std::future::Future;

use anyhow::Result;
use rmcp::{
    handler::server::{router::tool::ToolRouter, tool::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
    Error as McpError, ServiceExt,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::api;

/// Hard ceiling on a single MCP tool response. The protocol/client cap is
/// usually ~100 KB; we stop short of it so a too-broad query returns a clear
/// "narrow your query" error instead of a silently truncated payload that the
/// agent can't parse (issue #5).
const MCP_MAX_BYTES: usize = 80 * 1024;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TagArg {
    pub name: String,
    #[serde(default = "default_tag")]
    pub tag: String,
    /// Max VUIDs to include in the response (0 = all). Default 30.
    #[serde(default = "default_vuid_limit")]
    pub vuid_limit: usize,
    /// Skip this many VUIDs before the returned page (for paging).
    #[serde(default)]
    pub vuid_offset: usize,
    /// Set false to omit the VUID list entirely (counts are still reported).
    #[serde(default = "default_true")]
    pub include_vuids: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ExtensionsArg {
    #[serde(default = "default_tag")]
    pub tag: String,
    #[serde(default)]
    pub r#type: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    /// Compact mode (default): drop the long provides_commands/types/enums
    /// arrays, keep name/type/status/author/number. Set false for full rows.
    #[serde(default = "default_true")]
    pub compact: bool,
    /// Max extensions to return (0 = all). Default 100.
    #[serde(default = "default_ext_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DiffArg {
    pub v1: String,
    pub v2: String,
    #[serde(default)]
    pub entity: Option<String>,
    /// By default only bucket counts are returned (small). Set true for the
    /// full added/removed/changed lists — pair with `entity` to bound size.
    #[serde(default)]
    pub detail: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CallersArg {
    pub r#type: String,
    #[serde(default = "default_tag")]
    pub tag: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DepsArg {
    pub function: String,
    #[serde(default = "default_tag")]
    pub tag: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct VuidArg {
    pub vuid_id: String,
    #[serde(default = "default_tag")]
    pub tag: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchArg {
    pub query: String,
    #[serde(default = "default_tag")]
    pub tag: String,
    #[serde(default = "default_k")]
    pub k: usize,
    #[serde(default = "default_mode")]
    pub mode: String,
}

fn default_tag() -> String {
    "HEAD".into()
}
fn default_k() -> usize {
    10
}
fn default_mode() -> String {
    "hybrid".into()
}
fn default_vuid_limit() -> usize {
    30
}
fn default_ext_limit() -> usize {
    100
}
fn default_true() -> bool {
    true
}

/// Largest `k` we honour for search; keeps the response bounded.
const MAX_SEARCH_K: usize = 50;

/// Split an entity object's `vuids` array out into a paged, size-bounded
/// wrapper. Shared by `vk_get_function` / `vk_get_struct` so agents can page
/// through large VUID sets instead of overflowing the response (issue #5).
fn wrap_with_paged_vuids(
    entity: Value,
    entity_key: &str,
    tool: &str,
    limit: usize,
    offset: usize,
    include_vuids: bool,
) -> Value {
    let mut map = match entity {
        Value::Object(m) => m,
        other => return other,
    };
    let arr = match map.remove("vuids") {
        Some(Value::Array(a)) => a,
        _ => Vec::new(),
    };
    let total = arr.len();
    let (shown, truncated): (Vec<Value>, bool) = if !include_vuids {
        (Vec::new(), total > 0)
    } else {
        let off = offset.min(total);
        let end = if limit == 0 { total } else { (off + limit).min(total) };
        (arr[off..end].to_vec(), end < total || off > 0)
    };
    let next_offset = offset + shown.len();
    let hint = if !include_vuids && total > 0 {
        Some(format!(
            "VUIDs omitted (include_vuids=false). {total} available; call {tool} with include_vuids=true to fetch them."
        ))
    } else if truncated {
        Some(format!(
            "Showing {} of {total} VUIDs. Call {tool} again with vuid_offset={next_offset} for the next page.",
            shown.len()
        ))
    } else {
        None
    };
    json!({
        entity_key: Value::Object(map),
        "vuids": shown,
        "vuid_total": total,
        "vuid_offset": offset,
        "vuid_returned": shown.len(),
        "vuid_truncated": truncated,
        "hint": hint,
    })
}

#[derive(Clone)]
pub struct Vkquery {
    tool_router: ToolRouter<Vkquery>,
}

#[tool_router]
impl Vkquery {
    fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }

    #[tool(description = "Get a Vulkan command's signature, queue support, render-pass scope, VUIDs, and version availability. Large commands carry hundreds of VUIDs; the list is paged via vuid_limit (default 30) / vuid_offset, and the response reports vuid_total + vuid_truncated. Set include_vuids=false to omit them.")]
    async fn vk_get_function(&self, params: Parameters<TagArg>) -> Result<CallToolResult, McpError> {
        let a = params.0;
        json_or_err(api::get_function(&a.name, &a.tag).and_then(|info| {
            let full = serde_json::to_value(&info)?;
            Ok(wrap_with_paged_vuids(
                full,
                "function",
                "vk_get_function",
                a.vuid_limit,
                a.vuid_offset,
                a.include_vuids,
            ))
        }))
    }

    #[tool(description = "Get a Vulkan struct's members, structextends/extended_by, and VUIDs. The VUID list is paged via vuid_limit (default 30) / vuid_offset; the response reports vuid_total + vuid_truncated. Set include_vuids=false to omit them.")]
    async fn vk_get_struct(&self, params: Parameters<TagArg>) -> Result<CallToolResult, McpError> {
        let a = params.0;
        json_or_err(api::get_struct(&a.name, &a.tag).and_then(|info| {
            let full = serde_json::to_value(&info)?;
            Ok(wrap_with_paged_vuids(
                full,
                "struct",
                "vk_get_struct",
                a.vuid_limit,
                a.vuid_offset,
                a.include_vuids,
            ))
        }))
    }

    #[tool(description = "Filter the Vulkan extension catalogue by type (instance|device), author tag (KHR/EXT/…), and status. Compact by default (name/type/status/author/number only); set compact=false for full rows. limit caps the count (default 100).")]
    async fn vk_list_extensions(&self, params: Parameters<ExtensionsArg>) -> Result<CallToolResult, McpError> {
        let a = params.0;
        json_or_err(
            api::list_extensions(&a.tag, a.r#type.as_deref(), a.author.as_deref(), a.status.as_deref())
                .and_then(|exts| build_extensions_payload(&exts, a.compact, a.limit)),
        )
    }

    #[tool(description = "Compare two Vulkan-Docs git tags. By default returns only per-bucket counts (added/removed/changed) which stays small; pass detail=true (ideally with an entity filter) for the full lists.")]
    async fn vk_diff_versions(&self, params: Parameters<DiffArg>) -> Result<CallToolResult, McpError> {
        let a = params.0;
        json_or_err(
            crate::index::diff::diff_versions(&a.v1, &a.v2, a.entity.as_deref()).and_then(|report| {
                if a.detail {
                    Ok(serde_json::to_value(&report)?)
                } else {
                    Ok(diff_counts(&report)?)
                }
            }),
        )
    }

    #[tool(description = "Find every command and struct that consumes a given Vulkan type, following alias chains.")]
    async fn vk_find_callers(&self, params: Parameters<CallersArg>) -> Result<CallToolResult, McpError> {
        let a = params.0;
        json_or_err(api::find_callers(&a.r#type, &a.tag))
    }

    #[tool(description = "Get a Vulkan command's parent handle chain, required features, required + transitive extensions, pNext chain, and externsync params.")]
    async fn vk_find_dependencies(&self, params: Parameters<DepsArg>) -> Result<CallToolResult, McpError> {
        let a = params.0;
        json_or_err(api::find_dependencies(&a.function, &a.tag))
    }

    #[tool(description = "Look up a single VUID by id; returns its rule text, source file, and guard extensions.")]
    async fn vk_get_vuid(&self, params: Parameters<VuidArg>) -> Result<CallToolResult, McpError> {
        let a = params.0;
        json_or_err(api::get_vuid(&a.vuid_id, &a.tag))
    }

    #[tool(description = "Search Vulkan-Docs prose + VUID text. Modes: bm25 (lexical), embed (semantic, requires --features embed), hybrid (RRF fusion). k is clamped to [1, 50] to keep responses bounded.")]
    async fn vk_search_concept(&self, params: Parameters<SearchArg>) -> Result<CallToolResult, McpError> {
        let a = params.0;
        let k = a.k.clamp(1, MAX_SEARCH_K);
        json_or_err(api::search_concept(&a.query, &a.tag, k, &a.mode))
    }
}

/// Project an extension list to a compact (or full) JSON array, capped at
/// `limit` rows (0 = all). Compact keeps only the fields an agent needs to
/// triage; the heavy provides_* arrays are dropped.
fn build_extensions_payload(
    exts: &[crate::types::ExtensionInfo],
    compact: bool,
    limit: usize,
) -> anyhow::Result<Value> {
    let total = exts.len();
    let take = if limit == 0 { total } else { limit.min(total) };
    let rows: Vec<Value> = if compact {
        exts[..take]
            .iter()
            .map(|e| {
                json!({
                    "name": e.name,
                    "type": e.ty,
                    "status": e.status,
                    "author": e.author,
                    "number": e.number,
                })
            })
            .collect()
    } else {
        exts[..take]
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()?
    };
    Ok(json!({
        "extensions": rows,
        "total": total,
        "returned": take,
        "truncated": take < total,
        "compact": compact,
    }))
}

/// Per-bucket added/removed/changed counts for a diff report — the default
/// (small) shape returned by `vk_diff_versions`.
fn diff_counts(report: &crate::types::DiffReport) -> anyhow::Result<Value> {
    let v = serde_json::to_value(report)?;
    let bucket = |key: &str| -> Value {
        let b = v.get(key);
        let len = |field: &str| b.and_then(|x| x.get(field)).and_then(|a| a.as_array()).map(|a| a.len()).unwrap_or(0);
        json!({ "added": len("added"), "removed": len("removed"), "changed": len("changed") })
    };
    Ok(json!({
        "from_tag": report.from_tag,
        "to_tag": report.to_tag,
        "functions": bucket("functions"),
        "structs": bucket("structs"),
        "enums": bucket("enums"),
        "handles": bucket("handles"),
        "extensions": bucket("extensions"),
        "features": bucket("features"),
        "vuids": bucket("vuids"),
        "promoted": v.get("promoted").and_then(|a| a.as_array()).map(|a| a.len()).unwrap_or(0),
        "hint": "Counts only. Call vk_diff_versions with detail=true (and an entity filter) for full lists.",
    }))
}

#[tool_handler]
impl rmcp::ServerHandler for Vkquery {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "Query/retrieval layer over Khronos Vulkan-Docs. All tools accept `tag=<v1.x.y>` to pin against a specific git tag.".into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

/// Serialize a query result as JSON; turn errors into MCP error responses.
///
/// A final size net guards against any response that slips past the
/// per-tool paging (issue #5): if it would exceed `MCP_MAX_BYTES` we return a
/// clear "narrow your query" error instead of a payload the client will
/// truncate into unparseable JSON.
fn json_or_err<T: serde::Serialize>(result: anyhow::Result<T>) -> Result<CallToolResult, McpError> {
    match result {
        Ok(v) => match serde_json::to_string_pretty(&v) {
            Ok(s) if s.len() > MCP_MAX_BYTES => Ok(CallToolResult::error(vec![Content::text(format!(
                "Response is {} bytes, over the {} byte limit. Narrow the query or use pagination: vuid_limit/vuid_offset/include_vuids (function/struct), limit/compact (extensions), detail+entity (diff), or a smaller k (search).",
                s.len(),
                MCP_MAX_BYTES
            ))])),
            Ok(s) => Ok(CallToolResult::success(vec![Content::text(s)])),
            Err(e) => Err(McpError::internal_error(format!("json serialize: {e}"), None)),
        },
        Err(e) => Ok(CallToolResult::error(vec![Content::text(format!("{e}"))])),
    }
}

/// Block on a fresh tokio runtime to start the stdio MCP server. Returns
/// when the client disconnects.
pub fn run() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let service = Vkquery::new().serve(stdio()).await?;
        service.waiting().await?;
        Ok::<(), anyhow::Error>(())
    })
}

// Required by the macro infrastructure but useful as a unit test too.
#[allow(dead_code)]
fn _ensure_future_send<T: Future + Send>(_: T) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionInfo, Vuid, VuidKind};

    fn big_function(n: usize) -> FunctionInfo {
        let vuids = (0..n)
            .map(|i| Vuid {
                id: format!("VUID-vkCmdDraw-None-{i:05}"),
                entity: "vkCmdDraw".into(),
                param: "None".into(),
                kind: if i % 100 == 0 { VuidKind::Implicit } else { VuidKind::Explicit },
                text: "A valid pipeline must be bound to the pipeline bind point used by this command, and any parameters referenced by that pipeline must be valid.".into(),
                guard_extensions: vec!["VK_EXT_shader_object".into()],
                source_file: Some("chapters/commonvalidity/draw.adoc".into()),
            })
            .collect();
        FunctionInfo {
            name: "vkCmdDraw".into(),
            return_type: "void".into(),
            vuids,
            ..Default::default()
        }
    }

    #[test]
    fn paged_function_stays_under_mcp_limit() {
        let f = big_function(400);
        let full = serde_json::to_value(&f).unwrap();
        let wrapped =
            wrap_with_paged_vuids(full, "function", "vk_get_function", default_vuid_limit(), 0, true);
        let s = serde_json::to_string_pretty(&wrapped).unwrap();
        assert!(s.len() < MCP_MAX_BYTES, "paged response {} >= cap {}", s.len(), MCP_MAX_BYTES);
        assert_eq!(wrapped["vuid_total"], 400);
        assert_eq!(wrapped["vuid_returned"], 30);
        assert_eq!(wrapped["vuid_truncated"], true);
        // VUIDs must be lifted out of the entity object, not duplicated.
        assert!(wrapped["function"].get("vuids").is_none());
    }

    #[test]
    fn unpaged_full_vuids_would_overflow_without_paging() {
        // Sanity: the raw payload really is the problem paging solves.
        let f = big_function(400);
        let raw = serde_json::to_string_pretty(&f).unwrap();
        assert!(raw.len() > MCP_MAX_BYTES, "fixture not large enough: {}", raw.len());
    }

    #[test]
    fn include_vuids_false_omits_list_but_keeps_count() {
        let f = big_function(50);
        let full = serde_json::to_value(&f).unwrap();
        let wrapped =
            wrap_with_paged_vuids(full, "function", "vk_get_function", 30, 0, false);
        assert_eq!(wrapped["vuid_total"], 50);
        assert_eq!(wrapped["vuid_returned"], 0);
        assert_eq!(wrapped["vuid_truncated"], true);
        assert!(wrapped["vuids"].as_array().unwrap().is_empty());
    }

    #[test]
    fn paging_offset_returns_next_window() {
        let f = big_function(50);
        let full = serde_json::to_value(&f).unwrap();
        let wrapped =
            wrap_with_paged_vuids(full, "function", "vk_get_function", 10, 45, true);
        assert_eq!(wrapped["vuid_offset"], 45);
        assert_eq!(wrapped["vuid_returned"], 5); // only 5 left past offset 45
        assert_eq!(wrapped["vuid_truncated"], true);
    }
}
