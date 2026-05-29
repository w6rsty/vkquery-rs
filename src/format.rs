//! Human-readable rendering + VUID paging shared by the CLI and the MCP
//! server. The library API and the `--json` CLI path still return the full
//! structured payloads; these helpers exist so the *default* terminal output
//! and the MCP responses stay readable and within size limits.
//!
//! Big commands like `vkCmdDraw` carry ~380 VUIDs (~196 KB of JSON). Dumping
//! that to a terminal is unreadable (issue #2) and overflows the MCP response
//! limit (issue #5), so we summarise and page the VUID list by default.

use std::fmt::Write as _;

use crate::types::*;

/// How much VUID detail to include. `limit == 0` means "all".
#[derive(Debug, Clone, Copy)]
pub struct VuidPaging {
    pub limit: usize,
    pub offset: usize,
}

impl Default for VuidPaging {
    fn default() -> Self {
        Self { limit: 20, offset: 0 }
    }
}

/// A paged window over a VUID list, plus the counts needed to tell the user
/// what was elided.
pub struct VuidView<'a> {
    pub shown: Vec<&'a Vuid>,
    pub total: usize,
    pub explicit: usize,
    pub implicit: usize,
    pub offset: usize,
    pub truncated: bool,
}

/// Page a VUID slice. `limit == 0` returns everything from `offset` on.
pub fn page_vuids(vuids: &[Vuid], paging: VuidPaging) -> VuidView<'_> {
    let total = vuids.len();
    let explicit = vuids.iter().filter(|v| v.kind == VuidKind::Explicit).count();
    let implicit = total - explicit;
    let offset = paging.offset.min(total);
    let end = if paging.limit == 0 { total } else { (offset + paging.limit).min(total) };
    let shown: Vec<&Vuid> = vuids[offset..end].iter().collect();
    let truncated = end < total || offset > 0;
    VuidView { shown, total, explicit, implicit, offset, truncated }
}

fn truncate_text(text: &str, max: usize) -> String {
    let t = text.trim();
    if t.chars().count() <= max {
        return t.to_string();
    }
    let mut out: String = t.chars().take(max).collect();
    out.push('…');
    out
}

fn render_vuid_block(s: &mut String, view: &VuidView) {
    let _ = writeln!(
        s,
        "  VUIDs:         {} explicit, {} implicit  (total {})",
        view.explicit, view.implicit, view.total
    );
    if view.total == 0 {
        return;
    }
    let _ = writeln!(s);
    if view.shown.is_empty() {
        let _ = writeln!(s, "  (no VUIDs in this page; offset {} ≥ total {})", view.offset, view.total);
    } else {
        let span_end = view.offset + view.shown.len();
        let _ = writeln!(
            s,
            "  VUIDs {}–{} of {}:",
            view.offset + 1,
            span_end,
            view.total
        );
        for v in &view.shown {
            let _ = writeln!(s, "    [{}] {}", kind_tag(v.kind), v.id);
            let text = truncate_text(&v.text, 100);
            if !text.is_empty() {
                let _ = writeln!(s, "      {text}");
            }
        }
    }
    if view.truncated {
        let _ = writeln!(s);
        let _ = writeln!(
            s,
            "  Showing {} of {} VUIDs. Use --all-vuids for the full list, --vuid-offset <N> to page, or --json for the raw payload.",
            view.shown.len(),
            view.total
        );
    }
}

fn kind_tag(kind: VuidKind) -> &'static str {
    match kind {
        VuidKind::Explicit => "explicit",
        VuidKind::Implicit => "implicit",
    }
}

fn join_or_dash(items: &[String]) -> String {
    if items.is_empty() {
        "—".to_string()
    } else {
        items.join(", ")
    }
}

// ---- function -------------------------------------------------------------

pub fn function_summary(f: &FunctionInfo, paging: VuidPaging) -> String {
    let mut s = String::new();
    if let Some(from) = &f.aliased_from {
        let _ = writeln!(s, "{}  (alias of {})  →  {}", from, f.name, f.return_type);
    } else {
        let _ = writeln!(s, "{}  →  {}", f.name, f.return_type);
    }
    let _ = writeln!(s, "  Params ({}):", f.params.len());
    let name_w = f.params.iter().map(|p| p.name.len()).max().unwrap_or(0);
    for p in &f.params {
        let _ = writeln!(s, "    {:<name_w$} : {}", p.name, p.ty, name_w = name_w);
    }
    if !f.queues.is_empty() {
        let _ = writeln!(s, "  Queues:        {}", join_or_dash(&f.queues));
    }
    if let Some(rp) = &f.renderpass {
        let _ = writeln!(s, "  Render pass:   {rp}");
    }
    if !f.cmdbufferlevel.is_empty() {
        let _ = writeln!(s, "  Cmd level:     {}", join_or_dash(&f.cmdbufferlevel));
    }
    if !f.available_in.is_empty() {
        let _ = writeln!(s, "  Available in:  {}", join_or_dash(&f.available_in));
    }
    if !f.error_codes.is_empty() {
        let _ = writeln!(s, "  Error codes:   {}", join_or_dash(&f.error_codes));
    }
    let view = page_vuids(&f.vuids, paging);
    render_vuid_block(&mut s, &view);
    s
}

// ---- struct ---------------------------------------------------------------

pub fn struct_summary(st: &StructInfo, paging: VuidPaging) -> String {
    let mut s = String::new();
    if let Some(from) = &st.aliased_from {
        let _ = writeln!(s, "{}  (alias of {})  [{}]", from, st.name, st.category);
    } else {
        let _ = writeln!(s, "{}  [{}]", st.name, st.category);
    }
    let _ = writeln!(s, "  Members ({}):", st.members.len());
    let name_w = st.members.iter().map(|m| m.name.len()).max().unwrap_or(0);
    for m in &st.members {
        let _ = writeln!(s, "    {:<name_w$} : {}", m.name, m.ty, name_w = name_w);
    }
    if !st.structextends.is_empty() {
        let _ = writeln!(s, "  Extends:       {}", join_or_dash(&st.structextends));
    }
    if !st.extended_by.is_empty() {
        let _ = writeln!(s, "  Extended by:   {}", join_or_dash(&st.extended_by));
    }
    if !st.available_in.is_empty() {
        let _ = writeln!(s, "  Available in:  {}", join_or_dash(&st.available_in));
    }
    let view = page_vuids(&st.vuids, paging);
    render_vuid_block(&mut s, &view);
    s
}

// ---- extensions -----------------------------------------------------------

pub fn extensions_summary(list: &[ExtensionInfo], limit: usize) -> String {
    let mut s = String::new();
    let total = list.len();
    let shown = if limit == 0 { total } else { limit.min(total) };
    let _ = writeln!(s, "Extensions: {total} match(es)");
    if total == 0 {
        return s;
    }
    let name_w = list[..shown].iter().map(|e| e.name.len()).max().unwrap_or(0);
    for e in &list[..shown] {
        let ty = match e.ty {
            Some(ExtensionType::Instance) => "instance",
            Some(ExtensionType::Device) => "device",
            None => "—",
        };
        let status = format!("{:?}", e.status).to_lowercase();
        let _ = writeln!(
            s,
            "  {:<name_w$}  {:<8}  {:<10}  {}",
            e.name,
            ty,
            status,
            e.author.as_deref().unwrap_or("—"),
            name_w = name_w
        );
    }
    if shown < total {
        let _ = writeln!(s);
        let _ = writeln!(
            s,
            "  Showing {shown} of {total}. Use --limit 0 for all, or --json for the full payload."
        );
    }
    s
}

// ---- search ---------------------------------------------------------------

pub fn search_summary(hits: &[SearchHit]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "Search: {} result(s)", hits.len());
    for (i, h) in hits.iter().enumerate() {
        let loc = h.source_file.as_deref().unwrap_or("?");
        let anchor = h.section_anchor.as_deref().unwrap_or("");
        let _ = writeln!(
            s,
            "  {:>2}. [{:.3}] {}{}{}",
            i + 1,
            h.score,
            loc,
            if anchor.is_empty() { "" } else { "#" },
            anchor
        );
        let snippet = truncate_text(&h.snippet, 160);
        if !snippet.is_empty() {
            let _ = writeln!(s, "      {snippet}");
        }
    }
    s
}

// ---- callers --------------------------------------------------------------

pub fn callers_summary(c: &CallersResult) -> String {
    let mut s = String::new();
    if c.canonical_name != c.type_name {
        let _ = writeln!(s, "{}  (resolved to {})", c.type_name, c.canonical_name);
    } else {
        let _ = writeln!(s, "{}", c.type_name);
    }
    let _ = writeln!(s, "  Commands ({}): {}", c.commands.len(), join_or_dash(&c.commands));
    let _ = writeln!(s, "  Structs ({}):  {}", c.structs.len(), join_or_dash(&c.structs));
    s
}

// ---- deps -----------------------------------------------------------------

pub fn deps_summary(d: &DepGraph) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "{}", d.function);
    if !d.parent_handle_chain.is_empty() {
        let _ = writeln!(s, "  Handle chain:       {}", d.parent_handle_chain.join(" → "));
    }
    if !d.required_features.is_empty() {
        let _ = writeln!(s, "  Required features:  {}", join_or_dash(&d.required_features));
    }
    if !d.required_extensions.is_empty() {
        let _ = writeln!(s, "  Required exts:      {}", join_or_dash(&d.required_extensions));
    }
    if !d.transitive_extensions.is_empty() {
        let _ = writeln!(s, "  Transitive exts:    {}", join_or_dash(&d.transitive_extensions));
    }
    if !d.pnext_chain.is_empty() {
        let _ = writeln!(s, "  pNext extenders:");
        for (k, v) in &d.pnext_chain {
            let _ = writeln!(s, "    {k}: {}", v.join(", "));
        }
    }
    if !d.externsync_params.is_empty() {
        let _ = writeln!(s, "  externsync params:  {}", join_or_dash(&d.externsync_params));
    }
    if !d.queues.is_empty() {
        let _ = writeln!(s, "  Queues:             {}", join_or_dash(&d.queues));
    }
    s
}

// ---- vuid -----------------------------------------------------------------

pub fn vuid_summary(v: &VuidInfo) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "{}  [{}]", v.id, kind_tag(v.kind));
    let _ = writeln!(s, "  Entity:  {}", v.entity);
    if !v.param.is_empty() {
        let _ = writeln!(s, "  Param:   {}", v.param);
    }
    if !v.available_in.is_empty() {
        let _ = writeln!(s, "  In:      {}", join_or_dash(&v.available_in));
    }
    if let Some(src) = &v.source_file {
        let _ = writeln!(s, "  Source:  {src}");
    }
    let _ = writeln!(s);
    let _ = writeln!(s, "  {}", v.text.trim());
    s
}

// ---- diff -----------------------------------------------------------------

fn bucket_line(s: &mut String, label: &str, b: &DiffBucket) {
    if b.added.is_empty() && b.removed.is_empty() && b.changed.is_empty() {
        return;
    }
    let _ = writeln!(
        s,
        "  {:<12} +{} added  -{} removed  ~{} changed",
        label,
        b.added.len(),
        b.removed.len(),
        b.changed.len()
    );
}

pub fn diff_summary(r: &DiffReport) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "diff {} → {}", r.from_tag, r.to_tag);
    bucket_line(&mut s, "functions", &r.functions);
    bucket_line(&mut s, "structs", &r.structs);
    bucket_line(&mut s, "enums", &r.enums);
    bucket_line(&mut s, "handles", &r.handles);
    bucket_line(&mut s, "extensions", &r.extensions);
    bucket_line(&mut s, "features", &r.features);
    bucket_line(&mut s, "vuids", &r.vuids);
    if !r.promoted.is_empty() {
        let _ = writeln!(s, "  promoted:    {} entr(ies)", r.promoted.len());
    }
    let _ = writeln!(s);
    let _ = writeln!(s, "  Use --json for the full added/removed/changed lists.");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vuid(id: &str, kind: VuidKind) -> Vuid {
        Vuid {
            id: id.into(),
            entity: "vkCmdDraw".into(),
            param: "None".into(),
            kind,
            text: "A valid pipeline must be bound to the pipeline bind point used by this command".into(),
            guard_extensions: vec![],
            source_file: None,
        }
    }

    #[test]
    fn page_vuids_limits_and_reports_truncation() {
        let vs: Vec<Vuid> = (0..50)
            .map(|i| vuid(&format!("VUID-x-{i:02}"), if i % 10 == 0 { VuidKind::Implicit } else { VuidKind::Explicit }))
            .collect();
        let view = page_vuids(&vs, VuidPaging { limit: 20, offset: 0 });
        assert_eq!(view.shown.len(), 20);
        assert_eq!(view.total, 50);
        assert_eq!(view.implicit, 5);
        assert_eq!(view.explicit, 45);
        assert!(view.truncated);
    }

    #[test]
    fn page_vuids_all_when_limit_zero() {
        let vs: Vec<Vuid> = (0..50).map(|i| vuid(&format!("v{i}"), VuidKind::Explicit)).collect();
        let view = page_vuids(&vs, VuidPaging { limit: 0, offset: 0 });
        assert_eq!(view.shown.len(), 50);
        assert!(!view.truncated);
    }

    #[test]
    fn page_vuids_offset_past_end_is_safe() {
        let vs: Vec<Vuid> = (0..5).map(|i| vuid(&format!("v{i}"), VuidKind::Explicit)).collect();
        let view = page_vuids(&vs, VuidPaging { limit: 20, offset: 100 });
        assert!(view.shown.is_empty());
        assert_eq!(view.total, 5);
        assert!(view.truncated);
    }

    #[test]
    fn function_summary_is_compact_for_large_vuid_lists() {
        let mut f = FunctionInfo { name: "vkCmdDraw".into(), return_type: "void".into(), ..Default::default() };
        f.params = vec![Param { name: "commandBuffer".into(), ty: "VkCommandBuffer".into(), ..Default::default() }];
        f.queues = vec!["graphics".into()];
        f.vuids = (0..380).map(|i| vuid(&format!("VUID-vkCmdDraw-{i:03}"), VuidKind::Explicit)).collect();
        let out = function_summary(&f, VuidPaging::default());
        // The whole point of #2: stay readable. 380 VUIDs must not blow up
        // the summary past a screenful-ish bound.
        assert!(out.lines().count() < 60, "summary too long: {} lines", out.lines().count());
        assert!(out.contains("total 380"));
        assert!(out.contains("--all-vuids"));
    }
}
