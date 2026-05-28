//! Integration tests that require a working sibling Vulkan-Docs clone at
//! `..\Vulkan-Docs`. `pytest`-equivalent for the Python repo.

use std::path::PathBuf;

use vkquery::docs_source::DocsSource;
use vkquery::git::TagReader;

fn vulkan_docs_path() -> Option<PathBuf> {
    // Walk up from CARGO_MANIFEST_DIR looking for sibling Vulkan-Docs.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    while let Some(parent) = p.parent() {
        let sibling = parent.join("Vulkan-Docs");
        if sibling.join("xml").join("vk.xml").is_file() {
            return Some(sibling);
        }
        p = parent.to_path_buf();
    }
    None
}

fn skip_if_no_docs() -> Option<DocsSource> {
    match vulkan_docs_path() {
        Some(p) => Some(DocsSource::new(Some(p))),
        None => {
            eprintln!("skipping: no sibling Vulkan-Docs clone found");
            None
        }
    }
}

#[test]
fn cat_file_reads_vk_xml_blob() {
    let Some(source) = skip_if_no_docs() else { return };
    let mut reader = TagReader::open(&source).expect("open cat-file");
    let bytes = reader.read_blob("HEAD", "xml/vk.xml").expect("read vk.xml");
    let text = std::str::from_utf8(&bytes).expect("vk.xml is utf-8");
    assert!(text.contains("<registry>"), "vk.xml missing <registry>");
    assert!(text.contains("vkCmdDraw"), "vk.xml missing vkCmdDraw");
}

#[test]
fn cat_file_reads_two_blobs_in_a_row() {
    // Make sure the long-lived subprocess survives multiple reads — that's
    // the entire reason we batch instead of spawning per-blob.
    let Some(source) = skip_if_no_docs() else { return };
    let mut reader = TagReader::open(&source).expect("open cat-file");
    let xml = reader.read_blob("HEAD", "xml/vk.xml").expect("xml");
    let readme = reader.read_blob("HEAD", "README.adoc").expect("readme");
    assert!(xml.len() > readme.len(), "vk.xml should be larger than README");
}

#[test]
fn resolve_head_returns_40_char_sha() {
    let Some(source) = skip_if_no_docs() else { return };
    let sha = source.resolve_ref("HEAD").expect("resolve HEAD");
    assert_eq!(sha.len(), 40, "expected 40-char sha, got {sha:?}");
    assert!(sha.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn blob_sha_returns_short_sha_format() {
    let Some(source) = skip_if_no_docs() else { return };
    let sha = source.blob_sha("HEAD", "xml/vk.xml").expect("blob sha");
    assert_eq!(sha.len(), 40);
}

// ---- R2 build-shard end-to-end --------------------------------------------

#[test]
fn build_shard_end_to_end_head() {
    let Some(source) = skip_if_no_docs() else { return };
    let dir = tempfile::tempdir().expect("tmpdir");
    // Skip embeddings in this test; the embedding pipeline has its own
    // sanity test in `embeddings_sanity.rs` and a full HEAD embed takes
    // hours on CPU.
    std::env::set_var("VKQUERY_SKIP_EMBED", "1");
    let cache = vkquery::cache::Cache::new(Some(dir.path().to_path_buf()));
    let shard = vkquery::index::build::build_shard(&source, &cache, "HEAD", true)
        .expect("build_shard HEAD");
    // The 8 R2-required JSONs should be present.
    for name in &["functions", "structs", "handles", "enums", "extensions", "features", "aliases", "reverse"] {
        let path = shard.json_path(name);
        assert!(path.is_file(), "missing {}.json: {}", name, path.display());
    }
    // Manifest contains entity counts.
    let manifest = shard.load_manifest().expect("load manifest");
    let cmd_count = manifest
        .extra
        .get("command_count")
        .and_then(|v| v.as_u64())
        .expect("command_count");
    assert!(cmd_count > 500, "expected >500 commands in HEAD shard, got {cmd_count}");
}

// ---- Registry parser tag-replay -------------------------------------------

#[test]
fn parses_head_vk_xml() {
    let Some(source) = skip_if_no_docs() else { return };
    let mut reader = TagReader::open(&source).expect("open cat-file");
    let bytes = reader.read_blob("HEAD", "xml/vk.xml").expect("read vk.xml");
    let text = std::str::from_utf8(&bytes).expect("utf-8");
    let reg = vkquery::registry::parse_registry(text).expect("parse vk.xml");

    // Canonical sentinel entities — these have existed since 1.0.
    assert!(reg.types.contains_key("VkInstance"), "missing VkInstance");
    assert!(reg.types.contains_key("VkImage"), "missing VkImage");
    assert!(reg.types.contains_key("VkImageCreateInfo"), "missing VkImageCreateInfo");
    assert!(reg.commands.contains_key("vkCmdDraw"), "missing vkCmdDraw");
    assert!(reg.extensions.contains_key("VK_KHR_surface"), "missing VK_KHR_surface");
    assert!(reg.features.contains_key("VK_VERSION_1_0"), "missing VK_VERSION_1_0");
    assert!(reg.features.contains_key("VK_VERSION_1_4"), "missing VK_VERSION_1_4");

    // VkInstance is a dispatchable handle.
    let inst = reg.types.get("VkInstance").unwrap();
    assert_eq!(inst.category, "handle");
    assert!(inst.dispatchable);

    // vkCmdDraw has the graphics queue and 5 params. The registry stores
    // the raw `queues=` attribute; xml_index (R2) is what normalizes
    // `VK_QUEUE_GRAPHICS_BIT` → `graphics`.
    let draw = reg.commands.get("vkCmdDraw").unwrap();
    assert_eq!(draw.params.len(), 5, "vkCmdDraw should have 5 params, got {}", draw.params.len());
    assert!(
        draw.queues.iter().any(|q| q.contains("GRAPHICS") || q == "graphics"),
        "vkCmdDraw missing graphics queue, got {:?}",
        draw.queues
    );
    // First param is commandBuffer with externsync="true".
    assert_eq!(draw.params[0].name, "commandBuffer");
    assert_eq!(draw.params[0].externsync.as_deref(), Some("true"));

    // VkImageCreateInfo has at least 14 members.
    let ici = reg.types.get("VkImageCreateInfo").unwrap();
    assert_eq!(ici.category, "struct");
    assert!(ici.members.len() >= 14, "VkImageCreateInfo should have ≥14 members, got {}", ici.members.len());

    // Sanity: total counts are reasonable. HEAD has hundreds of commands.
    assert!(reg.commands.len() > 500, "expected >500 commands, got {}", reg.commands.len());
    assert!(reg.types.len() > 1500, "expected >1500 types, got {}", reg.types.len());
    assert!(reg.extensions.len() > 400, "expected >400 extensions, got {}", reg.extensions.len());
}

#[test]
fn parses_v1_3_250_vk_xml() {
    let Some(source) = skip_if_no_docs() else { return };
    // Skip if this tag isn't available locally; not all dev clones have it.
    if source.resolve_ref("v1.3.250").is_err() {
        eprintln!("skipping: v1.3.250 not available in local clone");
        return;
    }
    let mut reader = TagReader::open(&source).expect("open cat-file");
    let bytes = reader.read_blob("v1.3.250", "xml/vk.xml").expect("read v1.3.250 vk.xml");
    let text = std::str::from_utf8(&bytes).expect("utf-8");
    let reg = vkquery::registry::parse_registry(text).expect("parse v1.3.250 vk.xml");
    // VK_VERSION_1_4 should NOT be present in v1.3.250.
    assert!(!reg.features.contains_key("VK_VERSION_1_4"), "VK_VERSION_1_4 leaked into v1.3.250");
    assert!(reg.features.contains_key("VK_VERSION_1_3"));
    assert!(reg.commands.contains_key("vkCmdDraw"));
}

#[test]
fn parses_v1_0_40_core_vk_xml() {
    // Old-schema tag to exercise legacy repair. v1.0.40-core kept vk.xml at
    // `src/spec/vk.xml`, not `xml/vk.xml`.
    let Some(source) = skip_if_no_docs() else { return };
    if source.resolve_ref("v1.0.40-core").is_err() {
        eprintln!("skipping: v1.0.40-core not available locally");
        return;
    }
    let mut reader = TagReader::open(&source).expect("open cat-file");
    let bytes = match reader.read_blob("v1.0.40-core", "src/spec/vk.xml") {
        Ok(b) => b,
        Err(_) => match reader.read_blob("v1.0.40-core", "xml/vk.xml") {
            Ok(b) => b,
            Err(e) => {
                eprintln!("skipping: no vk.xml at v1.0.40-core: {e}");
                return;
            }
        },
    };
    let text = std::str::from_utf8(&bytes).expect("utf-8");
    let reg = vkquery::registry::parse_registry(text).expect("parse v1.0.40-core");
    assert!(reg.commands.contains_key("vkCmdDraw"));
    assert!(reg.features.contains_key("VK_VERSION_1_0"));
    // VK_VERSION_1_1 doesn't exist yet at this tag.
    assert!(!reg.features.contains_key("VK_VERSION_1_1"));
}

#[test]
fn list_tags_returns_v1_tags() {
    let Some(source) = skip_if_no_docs() else { return };
    let tags = source.list_tags("v1.*").expect("list_tags");
    // Sample expected tags; the clone may not have all of them but should have
    // many.
    assert!(tags.len() > 50, "expected >50 v1.x.y tags, got {}", tags.len());
    assert!(
        tags.iter().any(|t| t.starts_with("v1.")),
        "no v1.* tag in list_tags output"
    );
}
