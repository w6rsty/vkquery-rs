//! Pre-1.4 schema repair. Mirrors `_repair_legacy_schema` in `reg_loader.py`.
//!
//! Currently this is a no-op stub; real repair logic lands in R1.
//!
//! Known fix-ups we'll need (from the Python version):
//!  - Some pre-1.4 funcpointer `<type>` elements have `<name>` as a direct
//!    child rather than wrapping it in `<proto>`. Normalize so the parser
//!    can rely on one shape.

/// Apply in-place tree repairs for legacy schemas. Returns the (possibly
/// rewritten) XML text. Pass-through on modern schemas.
pub fn repair(xml: &str) -> String {
    // Heuristic: only old schemas. Real impl in R1.
    xml.to_string()
}
