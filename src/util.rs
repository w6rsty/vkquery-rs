//! Type-name normalization + asciidoc inline-markup stripper.
//!
//! Mirrors `vkquery.util` (Python). See `C:\dev\vkquery\src\vkquery\util.py`.

use regex::Regex;
use std::sync::OnceLock;

static PTR_RE: OnceLock<Regex> = OnceLock::new();
static CONST_RE: OnceLock<Regex> = OnceLock::new();

fn ptr_re() -> &'static Regex {
    PTR_RE.get_or_init(|| Regex::new(r"\s*\*+\s*$").unwrap())
}

fn const_re() -> &'static Regex {
    CONST_RE.get_or_init(|| Regex::new(r"^\s*const\s+").unwrap())
}

/// Strip trailing `*` and leading `const ` (repeatedly) from a type name.
pub fn normalize_type(raw: &str) -> String {
    let mut s: String = raw.trim().to_string();
    loop {
        let after_ptr = ptr_re().replace(&s, "").into_owned();
        let after_const = const_re().replace(&after_ptr, "").trim().to_string();
        if after_const == s {
            return after_const;
        }
        s = after_const;
    }
}

// ---- asciidoc inline-markup stripping --------------------------------------

struct Replacer {
    re: Regex,
    repl: &'static str,
}

static REPLACERS: OnceLock<Vec<Replacer>> = OnceLock::new();

fn replacers() -> &'static [Replacer] {
    REPLACERS
        .get_or_init(|| {
            vec![
                Replacer { re: Regex::new(r"<<[^,>]*,([^>]+)>>").unwrap(), repl: "$1" },
                Replacer { re: Regex::new(r"<<([^,>]+)>>").unwrap(), repl: "$1" },
                Replacer {
                    re: Regex::new(
                        r"\b(pname|sname|tname|ename|fname|elink|slink|tlink|flink|dlink|reflink|code|basetype|apiext|ftext|etext|stext):([A-Za-z0-9_]+)",
                    )
                    .unwrap(),
                    repl: "$2",
                },
                Replacer { re: Regex::new(r"`([^`]+)`").unwrap(), repl: "$1" },
                Replacer { re: Regex::new(r"\*\*([^*]+)\*\*").unwrap(), repl: "$1" },
                // Italic single-`*` / single-`_` markers must not eat
                // identifier characters around them (otherwise
                // `VK_FILTER_CUBIC_EXT` loses underscores) AND must not eat
                // the boundary whitespace (otherwise "set _n_ in" collapses
                // to "setnin"). Rust's `regex` crate has no lookbehind, so
                // capture the boundaries and put them back via $1..$3.
                Replacer {
                    re: Regex::new(r"(^|[^A-Za-z0-9_])\*([^*\s][^*]*?)\*([^A-Za-z0-9_]|$)")
                        .unwrap(),
                    repl: "$1$2$3",
                },
                Replacer {
                    re: Regex::new(r"(^|[^A-Za-z0-9_])_([^_\s][^_]*?)_([^A-Za-z0-9_]|$)")
                        .unwrap(),
                    repl: "$1$2$3",
                },
                Replacer { re: Regex::new(r"link:[^\[]+\[([^\]]+)\]").unwrap(), repl: "$1" },
            ]
        })
        .as_slice()
}

static WHITESPACE_RE: OnceLock<Regex> = OnceLock::new();

pub fn strip_asciidoc_markup(text: &str) -> String {
    let mut s = text.to_string();
    for r in replacers() {
        s = r.re.replace_all(&s, r.repl).into_owned();
    }
    let ws = WHITESPACE_RE.get_or_init(|| Regex::new(r"\s+").unwrap());
    ws.replace_all(&s, " ").trim().to_string()
}

// ---- extension-author parsing ----------------------------------------------

static EXTNAME_RE: OnceLock<Regex> = OnceLock::new();

pub fn extension_author(name: &str) -> Option<String> {
    let re = EXTNAME_RE.get_or_init(|| Regex::new(r"^VK_([A-Z]+)_").unwrap());
    re.captures(name).map(|c| c[1].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_const_and_pointer() {
        // Strip trailing `*+` and a single leading `const ` only. Inner
        // `const` (e.g. `pointer-to-const-pointer`) is preserved so type
        // identity remains distinct from the unqualified form.
        assert_eq!(normalize_type("const VkImage*"), "VkImage");
        assert_eq!(normalize_type("VkImage *"), "VkImage");
        assert_eq!(normalize_type("const char* const*"), "char* const");
        assert_eq!(normalize_type("uint32_t"), "uint32_t");
    }

    #[test]
    fn extension_author_extracts_vendor_tag() {
        assert_eq!(extension_author("VK_KHR_surface"), Some("KHR".to_string()));
        assert_eq!(extension_author("VK_EXT_debug_utils"), Some("EXT".to_string()));
        assert_eq!(extension_author("not_a_vk_ext"), None);
    }

    #[test]
    fn strip_markup_unwraps_backticks_and_xrefs() {
        assert_eq!(strip_asciidoc_markup("`foo`"), "foo");
        assert_eq!(strip_asciidoc_markup("<<foo,bar>>"), "bar");
        assert_eq!(strip_asciidoc_markup("pname:image"), "image");
        // identifier underscores must survive
        let s = strip_asciidoc_markup("VK_FILTER_CUBIC_EXT");
        assert_eq!(s, "VK_FILTER_CUBIC_EXT");
    }
}
