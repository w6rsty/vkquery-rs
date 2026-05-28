//! Typed model of `vk.xml` produced by `parse::parse_registry`.
//!
//! This replaces the role of `scripts/reg.py` from the Vulkan-Docs tree —
//! we own this parser, and the tag-replay test suite in `tests/integration.rs`
//! catches schema-drift the first time CI runs against a tag we never saw
//! before. See `docs/architecture.md` (when written) for the reasoning.

pub mod legacy;
pub mod parse;
pub mod schema;

pub use parse::parse_registry;
pub use schema::*;
