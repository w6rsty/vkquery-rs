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

use crate::api;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TagArg {
    pub name: String,
    #[serde(default = "default_tag")]
    pub tag: String,
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
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DiffArg {
    pub v1: String,
    pub v2: String,
    #[serde(default)]
    pub entity: Option<String>,
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

#[derive(Clone)]
pub struct Vkquery {
    tool_router: ToolRouter<Vkquery>,
}

#[tool_router]
impl Vkquery {
    fn new() -> Self {
        Self { tool_router: Self::tool_router() }
    }

    #[tool(description = "Get a Vulkan command's signature, queue support, render-pass scope, VUIDs, and version availability.")]
    async fn vk_get_function(&self, params: Parameters<TagArg>) -> Result<CallToolResult, McpError> {
        let a = params.0;
        json_or_err(api::get_function(&a.name, &a.tag))
    }

    #[tool(description = "Get a Vulkan struct's members, structextends/extended_by, and VUIDs.")]
    async fn vk_get_struct(&self, params: Parameters<TagArg>) -> Result<CallToolResult, McpError> {
        let a = params.0;
        json_or_err(api::get_struct(&a.name, &a.tag))
    }

    #[tool(description = "Filter the Vulkan extension catalogue by type (instance|device), author tag (KHR/EXT/…), and status.")]
    async fn vk_list_extensions(&self, params: Parameters<ExtensionsArg>) -> Result<CallToolResult, McpError> {
        let a = params.0;
        json_or_err(api::list_extensions(
            &a.tag,
            a.r#type.as_deref(),
            a.author.as_deref(),
            a.status.as_deref(),
        ))
    }

    #[tool(description = "Compare two Vulkan-Docs git tags: added/removed/changed/promoted entities. Optional entity kind filter.")]
    async fn vk_diff_versions(&self, params: Parameters<DiffArg>) -> Result<CallToolResult, McpError> {
        let a = params.0;
        json_or_err(crate::index::diff::diff_versions(&a.v1, &a.v2, a.entity.as_deref()))
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

    #[tool(description = "Search Vulkan-Docs prose + VUID text. Modes: bm25 (lexical), embed (semantic, requires --features embed), hybrid (RRF fusion).")]
    async fn vk_search_concept(&self, params: Parameters<SearchArg>) -> Result<CallToolResult, McpError> {
        let a = params.0;
        json_or_err(api::search_concept(&a.query, &a.tag, a.k, &a.mode))
    }
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
fn json_or_err<T: serde::Serialize>(result: anyhow::Result<T>) -> Result<CallToolResult, McpError> {
    match result {
        Ok(v) => match serde_json::to_string_pretty(&v) {
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
