//! Chapter section splitter — turns `chapters/*.adoc` into rows of
//! `{file, section_id, heading, heading_path, text, refpage_entities}`.
//!
//! Mirrors `vkquery.index.prose`. Heading levels track `=+`; `[[anchor]]` on
//! its own line sets the next section's id; `[open,refpage='X']` records
//! `X` as a refpage entity for the current section. Text is run through
//! `strip_asciidoc_markup` so the output is plain prose ready for
//! tokenization.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::git::TagReader;
use crate::util::strip_asciidoc_markup;

static HEADING_RE: OnceLock<Regex> = OnceLock::new();
static ANCHOR_INLINE: OnceLock<Regex> = OnceLock::new();
static REFPAGE_OPEN: OnceLock<Regex> = OnceLock::new();
static INCLUDE_RE: OnceLock<Regex> = OnceLock::new();
static IFDEF_RE: OnceLock<Regex> = OnceLock::new();
static ENDIF_RE: OnceLock<Regex> = OnceLock::new();
static SLUG_NON_ALPHA: OnceLock<Regex> = OnceLock::new();

fn heading_re() -> &'static Regex {
    HEADING_RE.get_or_init(|| Regex::new(r"^(=+)\s+(.+?)\s*$").unwrap())
}
fn anchor_inline() -> &'static Regex {
    ANCHOR_INLINE.get_or_init(|| Regex::new(r"^\[\[([^\]]+)\]\]$").unwrap())
}
fn refpage_open() -> &'static Regex {
    REFPAGE_OPEN.get_or_init(|| Regex::new(r"^\[open\s*,\s*refpage='([^']+)'").unwrap())
}
fn include_re() -> &'static Regex {
    INCLUDE_RE.get_or_init(|| Regex::new(r"^include::").unwrap())
}
fn ifdef_re() -> &'static Regex {
    IFDEF_RE.get_or_init(|| Regex::new(r"^if(n?)def::").unwrap())
}
fn endif_re() -> &'static Regex {
    ENDIF_RE.get_or_init(|| Regex::new(r"^endif::").unwrap())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Section {
    pub file: String,
    pub section_id: String,
    pub heading: String,
    #[serde(default)]
    pub heading_path: Vec<String>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub refpage_entities: Vec<String>,
}

fn slug(s: &str) -> String {
    let re = SLUG_NON_ALPHA.get_or_init(|| Regex::new(r"[^a-z0-9]+").unwrap());
    let lower = s.to_ascii_lowercase();
    re.replace_all(&lower, "-").trim_matches('-').to_string()
}

/// Split a single `.adoc` chapter into prose sections.
pub fn split_chapter(path: &str, text: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    let mut heading_stack: Vec<(usize, String)> = Vec::new();
    let mut pending_anchor: Option<String> = None;
    let mut cur: Option<Section> = None;
    let mut buf: Vec<String> = Vec::new();
    let mut refpages: Vec<String> = Vec::new();

    let flush = |cur: &mut Option<Section>, buf: &mut Vec<String>, refpages: &mut Vec<String>, sections: &mut Vec<Section>| {
        if let Some(mut s) = cur.take() {
            let joined = buf.iter().filter(|b| !b.is_empty()).cloned().collect::<Vec<_>>().join(" ");
            s.text = strip_asciidoc_markup(&joined);
            s.refpage_entities = std::mem::take(refpages);
            sections.push(s);
        }
        buf.clear();
    };

    for raw in text.lines() {
        let line = raw.trim_end();
        if ifdef_re().is_match(line) || endif_re().is_match(line) || include_re().is_match(line) {
            continue;
        }
        if let Some(c) = anchor_inline().captures(line.trim()) {
            pending_anchor = Some(c.get(1).unwrap().as_str().to_string());
            continue;
        }
        if let Some(c) = refpage_open().captures(line) {
            refpages.push(c.get(1).unwrap().as_str().to_string());
            continue;
        }
        if let Some(c) = heading_re().captures(line) {
            flush(&mut cur, &mut buf, &mut refpages, &mut sections);
            let level = c.get(1).unwrap().as_str().len();
            let heading = c.get(2).unwrap().as_str().to_string();
            heading_stack.retain(|(lv, _)| *lv < level);
            heading_stack.push((level, heading.clone()));
            let section_id = pending_anchor.take().unwrap_or_else(|| slug(&heading));
            cur = Some(Section {
                file: path.to_string(),
                section_id,
                heading,
                heading_path: heading_stack.iter().map(|(_, n)| n.clone()).collect(),
                ..Default::default()
            });
            continue;
        }
        if line.trim().starts_with("//") {
            continue;
        }
        if cur.is_none() {
            cur = Some(Section {
                file: path.to_string(),
                section_id: "intro".into(),
                heading: path.to_string(),
                ..Default::default()
            });
        }
        buf.push(line.to_string());
    }
    flush(&mut cur, &mut buf, &mut refpages, &mut sections);

    sections.retain(|s| !s.text.is_empty());
    sections
}

/// Walk every `chapters/*.adoc` blob at `refspec`, splitting each into
/// sections. Excludes `chapters/commonvalidity/` (those are include-only).
pub fn extract_sections(reader: &mut TagReader, refspec: &str) -> Result<Vec<Section>> {
    let paths = reader.list_adoc(refspec, "chapters/").context("list chapters")?;
    let mut sections: Vec<Section> = Vec::new();
    let mut blobs: BTreeMap<String, String> = BTreeMap::new();
    for path in &paths {
        if path.starts_with("chapters/commonvalidity/") {
            continue;
        }
        let bytes = reader
            .read_blob(refspec, path)
            .with_context(|| format!("read {path}"))?;
        let text = String::from_utf8_lossy(&bytes).into_owned();
        blobs.insert(path.clone(), text);
    }
    for (path, text) in &blobs {
        sections.extend(split_chapter(path, text));
    }
    Ok(sections)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_two_sections() {
        let text = "\
== Intro

Some intro prose.

== Second

Body of second section.
";
        let sections = split_chapter("chapters/x.adoc", text);
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].heading, "Intro");
        assert!(sections[0].text.contains("intro prose"));
        assert_eq!(sections[1].heading, "Second");
    }

    #[test]
    fn captures_refpage_entities() {
        let text = "\
== Heading

[open,refpage='vkCmdDraw',desc='draw',type='protos']
--
Body about vkCmdDraw.
--
";
        let sections = split_chapter("chapters/draw.adoc", text);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].refpage_entities, vec!["vkCmdDraw"]);
    }

    #[test]
    fn anchor_overrides_slug() {
        let text = "\
[[my-anchor]]
== Section Heading

Body.
";
        let sections = split_chapter("chapters/x.adoc", text);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].section_id, "my-anchor");
    }
}
