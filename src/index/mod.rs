//! Per-shard index builders. Each submodule corresponds to one or more JSON
//! files under `<shard>/`.

pub mod build;
pub mod diff;
pub mod prose;
pub mod reverse;
pub mod vuid_explicit;
pub mod vuid_implicit;
pub mod xml_index;
