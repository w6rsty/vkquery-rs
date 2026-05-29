//! Clap-derive CLI entry point. Mirrors `vkquery.cli`.

use anyhow::Result;
use clap::{Parser, Subcommand};

use std::sync::OnceLock;

use crate::cache::Cache;
use crate::config_info::enabled_features_string;
use crate::docs_source::DocsSource;

/// `--version` string: `0.1.0 (features: mcp, embed)`. clap prepends the
/// binary name, so the user sees `vkquery 0.1.0 (features: mcp, embed)`.
/// clap's `version`/`after_help` want a `&'static str`, so we build these
/// once and leak them through a `OnceLock`.
fn long_version() -> &'static str {
    static V: OnceLock<String> = OnceLock::new();
    V.get_or_init(|| {
        format!("{} (features: {})", env!("CARGO_PKG_VERSION"), enabled_features_string())
    })
}

/// `--help` footer line naming the compiled-in features, so a slim release
/// binary makes its missing capabilities obvious without running a query.
fn help_footer() -> &'static str {
    static H: OnceLock<String> = OnceLock::new();
    H.get_or_init(|| format!("Built with features: {}", enabled_features_string()))
}

#[derive(Parser, Debug)]
#[command(
    name = "vkquery",
    about = "Vulkan-Docs query layer",
    version = long_version(),
    after_help = help_footer()
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

/// Search modes available in this build. `embed`/`hybrid` require the
/// `embed` feature; a slim binary only offers lexical `bm25` so clap rejects
/// the unavailable modes at parse time instead of failing mid-query.
#[cfg(feature = "embed")]
const SEARCH_MODES: [&str; 3] = ["bm25", "embed", "hybrid"];
#[cfg(not(feature = "embed"))]
const SEARCH_MODES: [&str; 1] = ["bm25"];

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Lookup a function.
    Function {
        name: String,
        #[arg(long, default_value = "HEAD")]
        tag: String,
        #[arg(long)]
        json: bool,
    },
    /// Lookup a struct.
    Struct {
        name: String,
        #[arg(long, default_value = "HEAD")]
        tag: String,
        #[arg(long)]
        json: bool,
    },
    /// List extensions.
    Extensions {
        #[arg(long, default_value = "HEAD")]
        tag: String,
        #[arg(long, value_parser = ["instance", "device"])]
        r#type: Option<String>,
        #[arg(long)]
        author: Option<String>,
        #[arg(long, value_parser = ["active", "promoted", "deprecated", "obsoleted"])]
        status: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Find commands/structs consuming a type.
    Callers {
        r#type: String,
        #[arg(long, default_value = "HEAD")]
        tag: String,
        #[arg(long)]
        json: bool,
    },
    /// Find dependencies of a function.
    Deps {
        name: String,
        #[arg(long, default_value = "HEAD")]
        tag: String,
        #[arg(long)]
        json: bool,
    },
    /// Lookup a VUID by id.
    Vuid {
        vuid_id: String,
        #[arg(long, default_value = "HEAD")]
        tag: String,
        #[arg(long)]
        json: bool,
    },
    /// Diff two tags.
    Diff {
        v1: String,
        v2: String,
        #[arg(long, value_parser = ["functions", "structs", "enums", "handles", "extensions", "features", "vuids"])]
        entity: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Prose search.
    Search {
        query: String,
        #[arg(long, default_value = "HEAD")]
        tag: String,
        #[arg(short = 'k', long = "k", default_value_t = 10)]
        k: usize,
        #[cfg(feature = "embed")]
        #[arg(long, value_parser = SEARCH_MODES, default_value = "hybrid")]
        mode: String,
        #[cfg(not(feature = "embed"))]
        #[arg(long, value_parser = SEARCH_MODES, default_value = "bm25")]
        mode: String,
    },
    /// Index management.
    Index {
        #[command(subcommand)]
        action: IndexAction,
    },
    /// Manage the local Vulkan-Docs clone.
    Docs {
        #[command(subcommand)]
        action: DocsAction,
    },
    /// Inspect the local shard cache (location + installed shards).
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
    /// Print runtime configuration: cache dir, docs path, env vars, features.
    Config {
        #[arg(long)]
        json: bool,
    },
    /// Run the MCP stdio server.
    #[cfg(feature = "mcp")]
    Mcp,
}

#[derive(Subcommand, Debug)]
pub enum IndexAction {
    Build {
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        latest: Option<usize>,
        #[arg(long)]
        force: bool,
    },
    List,
    Gc {
        #[arg(long = "keep-last", default_value_t = 10)]
        keep_last: usize,
    },
}

#[derive(Subcommand, Debug)]
pub enum DocsAction {
    Update,
    Path,
    Tags,
}

#[derive(Subcommand, Debug)]
pub enum CacheAction {
    /// Print cache root path and list installed shards with sizes.
    Info,
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Function { name, tag, json } => {
            let info = crate::api::get_function(&name, &tag)?;
            emit(&info, json)
        }
        Cmd::Struct { name, tag, json } => {
            let info = crate::api::get_struct(&name, &tag)?;
            emit(&info, json)
        }
        Cmd::Extensions { tag, r#type, author, status, json } => {
            let exts = crate::api::list_extensions(&tag, r#type.as_deref(), author.as_deref(), status.as_deref())?;
            emit(&exts, json)
        }
        Cmd::Callers { r#type, tag, json } => {
            let info = crate::api::find_callers(&r#type, &tag)?;
            emit(&info, json)
        }
        Cmd::Deps { name, tag, json } => {
            let dg = crate::api::find_dependencies(&name, &tag)?;
            emit(&dg, json)
        }
        Cmd::Vuid { vuid_id, tag, json } => {
            let v = crate::api::get_vuid(&vuid_id, &tag)?;
            emit(&v, json)
        }
        Cmd::Diff { v1, v2, entity, json: _ } => {
            let report = crate::index::diff::diff_versions(&v1, &v2, entity.as_deref())?;
            emit(&report, true)
        }
        Cmd::Search { query, tag, k, mode } => {
            let hits = crate::api::search_concept(&query, &tag, k, &mode)?;
            emit(&hits, true)
        }
        Cmd::Index { action } => run_index(action),
        Cmd::Docs { action } => run_docs(action),
        Cmd::Cache { action } => run_cache(action),
        Cmd::Config { json } => run_config(json),
        #[cfg(feature = "mcp")]
        Cmd::Mcp => crate::mcp_server::run(),
    }
}

fn emit<T: serde::Serialize>(value: &T, _as_json: bool) -> Result<()> {
    // We always emit JSON for now (matching the Python CLI, which also
    // defaults to JSON). The `--json` flag is reserved for future
    // human-friendly output.
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

fn run_index(action: IndexAction) -> Result<()> {
    let source = DocsSource::new(None);
    let cache = Cache::new(None);
    match action {
        IndexAction::Build { tag, all, latest, force } => {
            if all {
                let mut tags = vec!["HEAD".to_string()];
                tags.extend(source.list_tags("v1.*")?);
                for t in tags {
                    crate::index::build::build_shard(&source, &cache, &t, force)?;
                }
            } else if let Some(n) = latest {
                let tags: Vec<_> = source.list_tags("v1.*")?.into_iter().take(n).collect();
                for t in tags {
                    crate::index::build::build_shard(&source, &cache, &t, force)?;
                }
            } else {
                let t = tag.unwrap_or_else(|| "HEAD".into());
                crate::index::build::build_shard(&source, &cache, &t, force)?;
            }
        }
        IndexAction::List => {
            let shards = cache.list_shards();
            println!("{}", serde_json::to_string_pretty(&shards)?);
        }
        IndexAction::Gc { keep_last } => gc(&cache, keep_last)?,
    }
    Ok(())
}

fn run_docs(action: DocsAction) -> Result<()> {
    let source = DocsSource::new(None);
    match action {
        DocsAction::Update => {
            source.update()?;
            println!("Updated {}", source.path.display());
        }
        DocsAction::Path => println!("{}", source.path.display()),
        DocsAction::Tags => {
            for t in source.list_tags("v1.*")? {
                println!("{t}");
            }
        }
    }
    Ok(())
}

fn gc(cache: &Cache, keep_last: usize) -> Result<()> {
    let shards = cache.list_shards();
    let mut ordered: Vec<_> = shards.iter().filter(|(k, _)| k.as_str() != "HEAD").collect();
    ordered.sort_by(|a, b| b.1.built_at.cmp(&a.1.built_at));
    let mut keep: std::collections::BTreeSet<String> =
        ordered.iter().take(keep_last).map(|(k, _)| (*k).clone()).collect();
    keep.insert("HEAD".into());
    let mut removed = vec![];
    for (tag, info) in shards.iter() {
        if keep.contains(tag) {
            continue;
        }
        let _ = std::fs::remove_dir_all(&info.shard_dir);
        removed.push(tag.clone());
    }
    let new: std::collections::BTreeMap<_, _> = shards
        .iter()
        .filter(|(k, _)| keep.contains(*k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    std::fs::write(&cache.tags_index_path, serde_json::to_string_pretty(&new)?)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "removed": removed,
            "kept": keep,
        }))?
    );
    Ok(())
}

fn run_config(json: bool) -> Result<()> {
    let snap = crate::config_info::snapshot();
    if json {
        println!("{}", crate::config_info::render_json(&snap)?);
    } else {
        print!("{}", crate::config_info::render_human(&snap));
    }
    Ok(())
}

fn run_cache(action: CacheAction) -> Result<()> {
    let cache = Cache::new(None);
    match action {
        CacheAction::Info => {
            let shards = cache.list_shards();
            let mut entries = Vec::with_capacity(shards.len());
            let mut total = 0u64;
            for (tag, info) in &shards {
                let size = dir_size(std::path::Path::new(&info.shard_dir)).unwrap_or(0);
                total += size;
                let sha_short =
                    &info.vkxml_sha[..info.vkxml_sha.len().min(12)];
                entries.push(serde_json::json!({
                    "tag": tag,
                    "shard_dir": info.shard_dir,
                    "commit_sha": info.commit_sha,
                    "vkxml_sha": info.vkxml_sha,
                    "vkxml_sha_short": sha_short,
                    "builder_version": info.builder_version,
                    "built_at": info.built_at,
                    "size_bytes": size,
                }));
            }
            let payload = serde_json::json!({
                "cache_root": cache.root.to_string_lossy(),
                "tags_index_path": cache.tags_index_path.to_string_lossy(),
                "shard_count": entries.len(),
                "total_size_bytes": total,
                "shards": entries,
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
    }
    Ok(())
}

fn dir_size(p: &std::path::Path) -> std::io::Result<u64> {
    if !p.is_dir() {
        return Ok(0);
    }
    let mut total = 0u64;
    for entry in std::fs::read_dir(p)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            total += meta.len();
        } else if meta.is_dir() {
            total += dir_size(&entry.path())?;
        }
    }
    Ok(total)
}
