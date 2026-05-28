//! Manages the on-disk clone of Vulkan-Docs.
//!
//! Mirrors `vkquery.docs_source` (Python). See
//! `C:\dev\vkquery\src\vkquery\docs_source.py`.

use anyhow::{anyhow, Context, Result};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const VULKAN_DOCS_URL: &str = "https://github.com/KhronosGroup/Vulkan-Docs.git";

/// `~/.cache/vkquery` (Linux/macOS) or `%LOCALAPPDATA%\vkquery` (Windows),
/// overridable with `VKQUERY_CACHE_DIR`.
pub fn default_cache_dir() -> PathBuf {
    if let Ok(p) = env::var("VKQUERY_CACHE_DIR") {
        return PathBuf::from(p);
    }
    #[cfg(windows)]
    {
        if let Ok(p) = env::var("LOCALAPPDATA") {
            return PathBuf::from(p).join("vkquery");
        }
        if let Some(home) = dirs::home_dir() {
            return home.join("vkquery");
        }
        return PathBuf::from(".vkquery");
    }
    #[cfg(not(windows))]
    {
        if let Ok(p) = env::var("XDG_CACHE_HOME") {
            return PathBuf::from(p).join("vkquery");
        }
        if let Some(home) = dirs::home_dir() {
            return home.join(".cache").join("vkquery");
        }
        PathBuf::from(".cache/vkquery")
    }
}

/// Where to find a Vulkan-Docs clone. Honors `VKQUERY_DOCS_PATH`, else looks
/// for a sibling directory next to the binary's `vkquery-rs` parent (dev
/// convenience), else falls back to `<cache>/Vulkan-Docs`.
pub fn default_docs_path() -> PathBuf {
    if let Ok(p) = env::var("VKQUERY_DOCS_PATH") {
        return PathBuf::from(p);
    }
    // Walk up from the current dir looking for a sibling `Vulkan-Docs`. This
    // matches the Python version's behavior of preferring an existing
    // checkout when running from inside the source tree.
    if let Ok(cwd) = env::current_dir() {
        let mut p: &Path = &cwd;
        loop {
            if let Some(parent) = p.parent() {
                let sibling = parent.join("Vulkan-Docs");
                if sibling.join("xml").join("vk.xml").is_file() {
                    return sibling;
                }
                p = parent;
            } else {
                break;
            }
        }
    }
    default_cache_dir().join("Vulkan-Docs")
}

/// A handle to the Vulkan-Docs clone on disk.
#[derive(Debug, Clone)]
pub struct DocsSource {
    pub path: PathBuf,
}

impl DocsSource {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path: path.unwrap_or_else(default_docs_path),
        }
    }

    pub fn is_cloned(&self) -> bool {
        self.path.join(".git").exists() && self.path.join("xml").join("vk.xml").is_file()
    }

    pub fn ensure_cloned(&self) -> Result<()> {
        if self.is_cloned() {
            return Ok(());
        }
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        tracing::info!("Cloning Vulkan-Docs into {}", self.path.display());
        let status = Command::new("git")
            .args([
                "clone",
                "--filter=blob:none",
                "--no-checkout",
                VULKAN_DOCS_URL,
            ])
            .arg(&self.path)
            .status()
            .context("running git clone")?;
        if !status.success() {
            return Err(anyhow!("git clone failed: {status}"));
        }
        self.git(&["checkout", "main"])?;
        Ok(())
    }

    pub fn update(&self) -> Result<()> {
        self.ensure_cloned()?;
        tracing::info!("Fetching latest tags & main from Vulkan-Docs");
        self.git(&["fetch", "--tags", "origin"])?;
        self.git(&["checkout", "main"])?;
        self.git(&["pull", "--ff-only", "origin", "main"])?;
        Ok(())
    }

    pub fn list_tags(&self, glob: &str) -> Result<Vec<String>> {
        self.ensure_cloned()?;
        let out = self.git(&["tag", "--list", glob, "--sort=-v:refname"])?;
        Ok(out.lines().filter(|s| !s.is_empty()).map(str::to_string).collect())
    }

    pub fn head_commit(&self) -> Result<String> {
        self.ensure_cloned()?;
        Ok(self.git(&["rev-parse", "HEAD"])?.trim().to_string())
    }

    /// Resolve a tag or "HEAD" to a commit SHA.
    pub fn resolve_ref(&self, refspec: &str) -> Result<String> {
        self.ensure_cloned()?;
        if refspec == "HEAD" {
            return self.head_commit();
        }
        let dereffed = format!("{refspec}^{{commit}}");
        if let Ok(out) = self.git(&["rev-parse", "--verify", &dereffed]) {
            return Ok(out.trim().to_string());
        }
        // Try fetching the tag explicitly, then re-resolve.
        let _ = self.git(&["fetch", "--depth=1", "origin", "tag", refspec])?;
        Ok(self.git(&["rev-parse", "--verify", &dereffed])?.trim().to_string())
    }

    /// sha of `path` at `refspec`. Used to content-hash the XML.
    pub fn blob_sha(&self, refspec: &str, path: &str) -> Result<String> {
        let spec = format!("{refspec}:{path}");
        Ok(self.git(&["rev-parse", &spec])?.trim().to_string())
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(args)
            .output()
            .with_context(|| format!("git {args:?}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(anyhow!("git {args:?} failed: {stderr}"));
        }
        Ok(String::from_utf8(out.stdout).context("git output not utf-8")?)
    }
}
