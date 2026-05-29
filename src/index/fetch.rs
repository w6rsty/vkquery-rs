//! `vkquery index fetch` — download a pre-built slim shard from the
//! `shards-latest` GitHub Release, verify its SHA-256, and extract it into
//! the local cache. Saves users the multi-minute first-query shard build
//! (and the manual `curl | tar` recipe — issue #3).
//!
//! Unlike a local build, fetch needs no Vulkan-Docs clone: the shard tarball
//! carries its own `manifest.json` (tag + commit + vk.xml blob SHA), which we
//! read after extraction to register the shard in `tags-index.json`.

use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};

use crate::cache::{sha256_hex, Cache, Manifest, Shard};

/// `owner/repo` the shard assets are published under. Derived from the
/// crate's `repository` metadata so it tracks a fork/rename automatically.
fn repo_slug() -> &'static str {
    env!("CARGO_PKG_REPOSITORY")
        .trim_end_matches('/')
        .trim_start_matches("https://github.com/")
}

fn asset_name(tag: &str) -> String {
    format!("vkquery-shard-{tag}-slim.tar.gz")
}

fn download_url(release: &str, asset: &str) -> String {
    format!("https://github.com/{}/releases/download/{release}/{asset}", repo_slug())
}

#[derive(Debug)]
pub struct FetchOutcome {
    pub tag: String,
    pub skipped: bool,
    pub downloaded_bytes: u64,
    pub vkxml_sha: String,
    pub shard_dir: PathBuf,
}

/// Is a shard for `tag` already extracted locally? We scan for any
/// `tags/<tag>/<sha>/manifest.json` rather than consulting tags-index.json so
/// a hand-extracted tarball still counts.
fn existing_shard_dir(cache: &Cache, tag: &str) -> Option<PathBuf> {
    let tag_dir = cache.root.join("tags").join(tag);
    let entries = std::fs::read_dir(&tag_dir).ok()?;
    for e in entries.flatten() {
        let p = e.path();
        if p.join("manifest.json").is_file() {
            return Some(p);
        }
    }
    None
}

/// Download + verify + extract the shard for `tag`. Idempotent unless
/// `force`: if a shard is already present it is left untouched.
pub fn fetch_shard(cache: &Cache, tag: &str, release: &str, force: bool) -> Result<FetchOutcome> {
    if !force {
        if let Some(dir) = existing_shard_dir(cache, tag) {
            let vkxml_sha = read_manifest(&dir).map(|m| m.vkxml_sha).unwrap_or_default();
            tracing::info!("shard for {tag} already present at {}, skipping", dir.display());
            return Ok(FetchOutcome {
                tag: tag.to_string(),
                skipped: true,
                downloaded_bytes: 0,
                vkxml_sha,
                shard_dir: dir,
            });
        }
    }

    let asset = asset_name(tag);
    let url = download_url(release, &asset);
    let sha_url = format!("{url}.sha256");

    // Expected digest (the .sha256 sidecar is just the hex digest).
    let expected = http_get_string(&sha_url)
        .with_context(|| format!("fetch checksum {sha_url}"))?
        .split_whitespace()
        .next()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("empty checksum at {sha_url}"))?;

    let bytes = http_get_bytes(&url).with_context(|| format!("download {url}"))?;
    let downloaded_bytes = bytes.len() as u64;

    let actual = sha256_hex(&bytes);
    if !actual.eq_ignore_ascii_case(&expected) {
        bail!("checksum mismatch for {asset}: expected {expected}, got {actual}");
    }

    std::fs::create_dir_all(&cache.root)
        .with_context(|| format!("mkdir {}", cache.root.display()))?;
    extract_tar_gz(&bytes, &cache.root)
        .with_context(|| format!("extract {asset} into {}", cache.root.display()))?;

    let shard_dir = existing_shard_dir(cache, tag)
        .ok_or_else(|| anyhow!("extracted archive did not contain tags/{tag}/<sha>/manifest.json"))?;
    let manifest = read_manifest(&shard_dir)
        .with_context(|| format!("read manifest under {}", shard_dir.display()))?;

    // Register in tags-index.json so `cache info` / `index list` see it,
    // without needing a Vulkan-Docs clone to recompute the path.
    let shard = Shard {
        tag: tag.to_string(),
        commit_sha: manifest.commit_sha.clone(),
        vkxml_sha: manifest.vkxml_sha.clone(),
        root: shard_dir.clone(),
    };
    cache.update_index(&shard)?;

    Ok(FetchOutcome {
        tag: tag.to_string(),
        skipped: false,
        downloaded_bytes,
        vkxml_sha: manifest.vkxml_sha,
        shard_dir,
    })
}

/// List the shard tags published on `release`, parsed from the release's
/// asset names (`vkquery-shard-<tag>-slim.tar.gz`). Used by `--all`.
pub fn list_remote_tags(release: &str) -> Result<Vec<String>> {
    let api = format!("https://api.github.com/repos/{}/releases/tags/{release}", repo_slug());
    let body = http_get_string(&api).with_context(|| format!("GET {api}"))?;
    let json: serde_json::Value = serde_json::from_str(&body).context("parse release JSON")?;
    let assets = json
        .get("assets")
        .and_then(|a| a.as_array())
        .ok_or_else(|| anyhow!("release {release} has no assets array"))?;
    let mut tags = Vec::new();
    for a in assets {
        let Some(name) = a.get("name").and_then(|n| n.as_str()) else { continue };
        if let Some(tag) = name
            .strip_prefix("vkquery-shard-")
            .and_then(|s| s.strip_suffix("-slim.tar.gz"))
        {
            tags.push(tag.to_string());
        }
    }
    tags.sort();
    tags.dedup();
    Ok(tags)
}

// ---- helpers --------------------------------------------------------------

fn read_manifest(shard_dir: &Path) -> Result<Manifest> {
    let bytes = std::fs::read(shard_dir.join("manifest.json"))
        .with_context(|| format!("read {}", shard_dir.join("manifest.json").display()))?;
    Ok(serde_json::from_slice(&bytes)?)
}

/// GitHub requires a User-Agent on API requests; send one everywhere.
fn agent_get(url: &str) -> ureq::Request {
    ureq::get(url).set(
        "User-Agent",
        concat!("vkquery/", env!("CARGO_PKG_VERSION")),
    )
}

fn http_get_string(url: &str) -> Result<String> {
    let resp = agent_get(url).call().with_context(|| format!("GET {url}"))?;
    if resp.status() != 200 {
        bail!("GET {url} returned status {}", resp.status());
    }
    Ok(resp.into_string()?)
}

/// Download with a TTY-aware progress bar (silenced under pipes /
/// `VKQUERY_NO_PROGRESS=1`, matching the shard-build bars).
fn http_get_bytes(url: &str) -> Result<Vec<u8>> {
    let resp = agent_get(url).call().with_context(|| format!("GET {url}"))?;
    if resp.status() != 200 {
        bail!("GET {url} returned status {}", resp.status());
    }
    let total: u64 = resp
        .header("Content-Length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let show = std::io::stderr().is_terminal() && std::env::var_os("VKQUERY_NO_PROGRESS").is_none();
    let pb = if show {
        let pb = if total > 0 {
            let pb = ProgressBar::new(total);
            pb.set_style(
                ProgressStyle::with_template(
                    "  {bar:32.cyan/blue} {bytes}/{total_bytes} ({eta}) downloading shard",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("##-"),
            );
            pb
        } else {
            let pb = ProgressBar::new_spinner();
            pb.enable_steady_tick(Duration::from_millis(120));
            pb.set_message("downloading shard");
            pb
        };
        pb
    } else {
        ProgressBar::hidden()
    };

    let mut reader = resp.into_reader();
    let mut buf = Vec::with_capacity(total as usize);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut chunk).context("read response body")?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        pb.set_position(buf.len() as u64);
    }
    pb.finish_and_clear();
    Ok(buf)
}

fn extract_tar_gz(bytes: &[u8], dest_root: &Path) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    // Entries are rooted at `tags/<tag>/...`; unpack relative to the cache
    // root so we recreate `<root>/tags/<tag>/<sha>/...`.
    archive.unpack(dest_root).context("unpack tar")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_slug_is_owner_repo() {
        let slug = repo_slug();
        assert!(!slug.starts_with("http"), "slug still has scheme: {slug}");
        assert!(slug.contains('/'), "slug not owner/repo: {slug}");
        assert_eq!(slug, "w6rsty/vkquery-rs");
    }

    #[test]
    fn asset_and_url_shapes() {
        assert_eq!(asset_name("v1.4.352"), "vkquery-shard-v1.4.352-slim.tar.gz");
        let u = download_url("shards-latest", &asset_name("HEAD"));
        assert_eq!(
            u,
            "https://github.com/w6rsty/vkquery-rs/releases/download/shards-latest/vkquery-shard-HEAD-slim.tar.gz"
        );
    }

    #[test]
    fn round_trips_a_tar_gz_into_cache_layout() {
        use std::io::Write;
        // Build an in-memory tar.gz shaped like the shards.yml output:
        // tags/HEAD/<sha>/manifest.json
        let mut tar_buf = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut tar_buf, flate2::Compression::fast());
            let mut builder = tar::Builder::new(enc);
            let manifest = br#"{"tag":"HEAD","commit_sha":"abc","vkxml_sha":"0123456789abcdef","builder_version":"test","built_at":0}"#;
            let mut header = tar::Header::new_gnu();
            header.set_size(manifest.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "tags/HEAD/0123456789ab/manifest.json", &manifest[..])
                .unwrap();
            builder.into_inner().unwrap().finish().unwrap();
        }

        let tmp = tempfile::tempdir().unwrap();
        extract_tar_gz(&tar_buf, tmp.path()).unwrap();
        let extracted = tmp.path().join("tags/HEAD/0123456789ab/manifest.json");
        assert!(extracted.is_file(), "manifest not extracted to expected path");

        let cache = Cache::new(Some(tmp.path().to_path_buf()));
        let dir = existing_shard_dir(&cache, "HEAD").expect("shard dir found");
        let m = read_manifest(&dir).unwrap();
        assert_eq!(m.vkxml_sha, "0123456789abcdef");
        let _ = writeln!(std::io::sink(), "ok");
    }
}
