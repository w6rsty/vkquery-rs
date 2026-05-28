//! Shard layout + freshness checks. Mirrors `vkquery.cache`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::docs_source::{default_cache_dir, DocsSource};

/// Bumped to `0.2.0-rust` so we can distinguish Rust-built from Python-built
/// shards. The freshness check tolerates either direction — both
/// implementations can refresh the other.
pub const BUILDER_VERSION: &str = "0.2.0-rust";

/// One (tag, content-hash) snapshot on disk.
#[derive(Debug, Clone)]
pub struct Shard {
    pub tag: String,
    pub commit_sha: String,
    pub vkxml_sha: String,
    pub root: PathBuf,
}

impl Shard {
    pub fn manifest_path(&self) -> PathBuf {
        self.root.join("manifest.json")
    }

    pub fn json_path(&self, name: &str) -> PathBuf {
        self.root.join(format!("{name}.json"))
    }

    pub fn prose_dir(&self) -> PathBuf {
        self.root.join("prose")
    }

    pub fn bm25_dir(&self) -> PathBuf {
        self.root.join("bm25")
    }

    pub fn embeddings_dir(&self) -> PathBuf {
        self.root.join("embeddings")
    }

    pub fn exists(&self) -> bool {
        self.manifest_path().is_file()
    }

    pub fn has(&self, names: &[&str]) -> bool {
        names.iter().all(|n| self.json_path(n).is_file())
    }

    pub fn write_manifest(&self, extra: Option<BTreeMap<String, serde_json::Value>>) -> Result<()> {
        let mut data = serde_json::Map::new();
        data.insert("tag".into(), self.tag.clone().into());
        data.insert("commit_sha".into(), self.commit_sha.clone().into());
        data.insert("vkxml_sha".into(), self.vkxml_sha.clone().into());
        data.insert("builder_version".into(), BUILDER_VERSION.into());
        data.insert("built_at".into(), now_secs().into());
        if let Some(map) = extra {
            for (k, v) in map {
                data.insert(k, v);
            }
        }
        std::fs::create_dir_all(&self.root)
            .with_context(|| format!("mkdir {}", self.root.display()))?;
        let txt = serde_json::to_string_pretty(&serde_json::Value::Object(data))?;
        std::fs::write(self.manifest_path(), txt)
            .with_context(|| format!("write {}", self.manifest_path().display()))?;
        Ok(())
    }

    pub fn load_manifest(&self) -> Result<Manifest> {
        let bytes = std::fs::read(self.manifest_path())
            .with_context(|| format!("read {}", self.manifest_path().display()))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn write_json<T: Serialize>(&self, name: &str, payload: &T) -> Result<()> {
        std::fs::create_dir_all(&self.root)
            .with_context(|| format!("mkdir {}", self.root.display()))?;
        // Match Python's `json.dumps(..., indent=2, sort_keys=True)`: pretty
        // + sorted keys. `serde_json::Value` preserves whatever order the
        // caller supplied, so callers must populate sorted Maps (e.g. by
        // building a `BTreeMap<String, _>` or sorted `serde_json::Map`).
        let txt = serde_json::to_string_pretty(payload)?;
        std::fs::write(self.json_path(name), txt)
            .with_context(|| format!("write {}", self.json_path(name).display()))?;
        Ok(())
    }

    pub fn read_json<T: for<'de> Deserialize<'de>>(&self, name: &str) -> Result<T> {
        let bytes = std::fs::read(self.json_path(name))
            .with_context(|| format!("read {}", self.json_path(name).display()))?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub tag: String,
    pub commit_sha: String,
    pub vkxml_sha: String,
    pub builder_version: String,
    pub built_at: u64,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShardIndexEntry {
    pub shard_dir: String,
    pub commit_sha: String,
    pub vkxml_sha: String,
    pub builder_version: String,
    pub built_at: u64,
}

/// Top-level cache directory + shard resolver.
#[derive(Debug, Clone)]
pub struct Cache {
    pub root: PathBuf,
    pub tags_index_path: PathBuf,
}

impl Cache {
    pub fn new(root: Option<PathBuf>) -> Self {
        let root = root.unwrap_or_else(default_cache_dir);
        let tags_index_path = root.join("tags-index.json");
        Self { root, tags_index_path }
    }

    pub fn shard_for(&self, source: &DocsSource, tag: &str) -> Result<Shard> {
        let commit_sha = source.resolve_ref(tag)?;
        let vkxml_sha = source.blob_sha(tag, "xml/vk.xml")?;
        let root = self
            .root
            .join("tags")
            .join(safe_tag(tag))
            .join(&vkxml_sha[..12]);
        Ok(Shard { tag: tag.to_string(), commit_sha, vkxml_sha, root })
    }

    pub fn is_fresh(&self, shard: &Shard, required: &[&str]) -> bool {
        if !shard.exists() {
            return false;
        }
        let manifest = match shard.load_manifest() {
            Ok(m) => m,
            Err(_) => return false,
        };
        // We accept either Python-built or Rust-built shards if the
        // content-hash matches. The version tag is informational here.
        if manifest.vkxml_sha != shard.vkxml_sha {
            return false;
        }
        if !shard.has(required) {
            return false;
        }
        true
    }

    pub fn update_index(&self, shard: &Shard) -> Result<()> {
        let mut idx = self.load_index();
        idx.insert(
            shard.tag.clone(),
            ShardIndexEntry {
                shard_dir: shard.root.to_string_lossy().into_owned(),
                commit_sha: shard.commit_sha.clone(),
                vkxml_sha: shard.vkxml_sha.clone(),
                builder_version: BUILDER_VERSION.into(),
                built_at: now_secs(),
            },
        );
        if let Some(p) = self.tags_index_path.parent() {
            std::fs::create_dir_all(p).ok();
        }
        let txt = serde_json::to_string_pretty(&idx)?;
        std::fs::write(&self.tags_index_path, txt)?;
        Ok(())
    }

    pub fn list_shards(&self) -> BTreeMap<String, ShardIndexEntry> {
        self.load_index()
    }

    fn load_index(&self) -> BTreeMap<String, ShardIndexEntry> {
        let Ok(bytes) = std::fs::read(&self.tags_index_path) else {
            return BTreeMap::new();
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }
}

fn safe_tag(tag: &str) -> String {
    if tag
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
    {
        return tag.to_string();
    }
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(tag.as_bytes());
    hex::encode(&hasher.finalize()[..8])
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// SHA-256 of a byte slice, hex-encoded. Used to content-hash files outside git.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_tag_passes_through_clean_names() {
        assert_eq!(safe_tag("v1.4.352"), "v1.4.352");
        assert_eq!(safe_tag("HEAD"), "HEAD");
    }

    #[test]
    fn shard_paths() {
        let s = Shard {
            tag: "v1.4.352".into(),
            commit_sha: "deadbeef".into(),
            vkxml_sha: "abcdef0123456789".into(),
            root: std::path::PathBuf::from("/tmp/v"),
        };
        assert!(s.json_path("functions").ends_with("functions.json"));
    }
}
