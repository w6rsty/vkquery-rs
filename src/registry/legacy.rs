//! Pre-1.4 schema repair: string-level fix-ups applied to `xml/vk.xml`
//! before `roxmltree` parsing.
//!
//! Currently a no-op pass-through. Known fix-ups that will need to land
//! here when they bite:
//!  - Some pre-1.4 funcpointer `<type>` elements have `<name>` as a direct
//!    child rather than wrapping it in `<proto>`. Normalize so the parser
//!    can rely on one shape.

/// Apply in-place tree repairs for legacy schemas. Returns the (possibly
/// rewritten) XML text. Pass-through on modern schemas.
pub fn repair(xml: &str) -> String {
    xml.to_string()
}
