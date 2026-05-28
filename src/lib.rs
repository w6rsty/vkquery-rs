//! vkquery — Rust port. Reference implementation lives in `C:\dev\vkquery`.
//!
//! Public surface:
//! - [`api`] — the 8 query primitives.
//! - [`types`] — the data structs they return.
//! - [`cli`] — `clap` entry point (used by the `vkquery` binary).
//! - [`cache`] — shard layout / freshness.
//! - [`docs_source`] — Vulkan-Docs clone lifecycle.

pub mod api;
pub mod cache;
pub mod cli;
pub mod config_info;
pub mod docs_source;
pub mod git;
pub mod index;
pub mod registry;
pub mod search;
pub mod types;
pub mod util;

#[cfg(feature = "mcp")]
pub mod mcp_server;
