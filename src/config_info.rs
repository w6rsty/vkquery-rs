//! Build-time and runtime configuration snapshot — backs the `vkquery config`
//! command and the `--version` long-version footer.
//!
//! Everything here is read-only. We do not write a config file; "set" is
//! achieved by exporting the documented environment variables.

use serde::Serialize;
use std::path::PathBuf;

use crate::docs_source::{default_cache_dir, default_docs_path};

/// Compile-time list of cargo features that affect runtime behaviour.
pub fn enabled_features() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    if cfg!(feature = "mcp") {
        out.push("mcp");
    }
    if cfg!(feature = "embed") {
        out.push("embed");
    }
    if cfg!(feature = "cuda") {
        out.push("cuda");
    }
    if cfg!(feature = "cudnn") {
        out.push("cudnn");
    }
    if cfg!(feature = "mkl") {
        out.push("mkl");
    }
    if cfg!(feature = "accelerate") {
        out.push("accelerate");
    }
    if cfg!(feature = "metal") {
        out.push("metal");
    }
    out
}

/// Comma-joined feature list. Stable shape used by `--version` and tests.
pub fn enabled_features_string() -> String {
    let feats = enabled_features();
    if feats.is_empty() {
        "(none)".to_string()
    } else {
        feats.join(", ")
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EnvVar {
    pub name: &'static str,
    pub value: Option<String>,
    pub description: &'static str,
    pub default_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigSnapshot {
    pub version: &'static str,
    pub features: Vec<&'static str>,
    pub cache_dir: PathBuf,
    pub docs_path: PathBuf,
    pub env: Vec<EnvVar>,
}

/// The canonical list of env vars vkquery honours. Keep in sync with
/// `tests/config_env_drift.rs` — the test greps `src/` to make sure no
/// `VKQUERY_*` reference exists that we forgot to document here.
pub fn snapshot() -> ConfigSnapshot {
    let env = vec![
        EnvVar {
            name: "VKQUERY_CACHE_DIR",
            value: std::env::var("VKQUERY_CACHE_DIR").ok(),
            description: "override the shard cache root",
            default_hint: Some(default_cache_dir().display().to_string()),
        },
        EnvVar {
            name: "VKQUERY_DOCS_PATH",
            value: std::env::var("VKQUERY_DOCS_PATH").ok(),
            description: "override the Vulkan-Docs clone path",
            default_hint: Some(default_docs_path().display().to_string()),
        },
        EnvVar {
            name: "VKQUERY_SKIP_EMBED",
            value: std::env::var("VKQUERY_SKIP_EMBED").ok(),
            description: "skip embeddings during `index build` (slim shards)",
            default_hint: None,
        },
        EnvVar {
            name: "VKQUERY_EMBED_LIMIT",
            value: std::env::var("VKQUERY_EMBED_LIMIT").ok(),
            description: "cap embedding rows for fast iteration (embed feature only)",
            default_hint: None,
        },
        EnvVar {
            name: "VKQUERY_NO_PROGRESS",
            value: std::env::var("VKQUERY_NO_PROGRESS").ok(),
            description: "silence progress bars (auto under pipes / non-TTY)",
            default_hint: None,
        },
    ];
    ConfigSnapshot {
        version: env!("CARGO_PKG_VERSION"),
        features: enabled_features(),
        cache_dir: default_cache_dir(),
        docs_path: default_docs_path(),
        env,
    }
}

pub fn render_human(snap: &ConfigSnapshot) -> String {
    use std::fmt::Write as _;
    let mut s = String::new();
    let _ = writeln!(s, "vkquery {}", snap.version);
    let _ = writeln!(s, "Features:    {}", enabled_features_string());
    let _ = writeln!(s, "Cache dir:   {}", snap.cache_dir.display());
    let _ = writeln!(s, "Docs path:   {}", snap.docs_path.display());
    let _ = writeln!(s);
    let _ = writeln!(s, "Environment variables:");
    let name_w = snap.env.iter().map(|e| e.name.len()).max().unwrap_or(0);
    let value_w = snap
        .env
        .iter()
        .map(|e| e.value.as_deref().map(|v| v.len()).unwrap_or("(unset)".len()))
        .max()
        .unwrap_or(0);
    for e in &snap.env {
        let value = e.value.as_deref().unwrap_or("(unset)");
        let _ = writeln!(
            s,
            "  {name:<name_w$}  {value:<value_w$}  {desc}",
            name = e.name,
            value = value,
            desc = e.description,
            name_w = name_w,
            value_w = value_w,
        );
        if e.value.is_none() {
            if let Some(hint) = &e.default_hint {
                let _ = writeln!(
                    s,
                    "  {:<name_w$}  {:<value_w$}    (default: {})",
                    "",
                    "",
                    hint,
                    name_w = name_w,
                    value_w = value_w,
                );
            }
        }
    }
    let _ = writeln!(s);
    let _ = writeln!(
        s,
        "Set values via environment variables, e.g."
    );
    let _ = writeln!(
        s,
        "  PowerShell: $env:VKQUERY_CACHE_DIR = \"D:\\vkcache\""
    );
    let _ = writeln!(
        s,
        "  bash/zsh:   export VKQUERY_CACHE_DIR=/path/to/vkcache"
    );
    s
}

pub fn render_json(snap: &ConfigSnapshot) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(snap)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_lists_documented_env_vars() {
        let snap = snapshot();
        let names: Vec<_> = snap.env.iter().map(|e| e.name).collect();
        assert!(names.contains(&"VKQUERY_CACHE_DIR"));
        assert!(names.contains(&"VKQUERY_DOCS_PATH"));
        assert!(names.contains(&"VKQUERY_SKIP_EMBED"));
        assert!(names.contains(&"VKQUERY_EMBED_LIMIT"));
        assert!(names.contains(&"VKQUERY_NO_PROGRESS"));
    }

    #[test]
    fn enabled_features_is_subset_of_known() {
        let known = [
            "mcp",
            "embed",
            "cuda",
            "cudnn",
            "mkl",
            "accelerate",
            "metal",
        ];
        for f in enabled_features() {
            assert!(known.contains(&f), "unexpected feature: {f}");
        }
    }

    #[test]
    fn human_render_is_nonempty_and_starts_with_version() {
        let snap = snapshot();
        let out = render_human(&snap);
        assert!(out.starts_with("vkquery "));
        assert!(out.contains("Features:"));
        assert!(out.contains("VKQUERY_CACHE_DIR"));
    }

    #[test]
    fn json_render_is_valid() {
        let snap = snapshot();
        let s = render_json(&snap).unwrap();
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v.get("version").is_some());
        assert!(v.get("features").is_some());
        assert!(v.get("env").and_then(|e| e.as_array()).is_some());
    }
}
