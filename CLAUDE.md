# CLAUDE.md — vkquery (Rust)

Agent entrypoint for `vkquery-rs`. Read this first.

## What this repo is

`vkquery-rs` is a single-binary query/retrieval layer over Khronos
Vulkan-Docs, written in Rust. 8 query primitives, all version-pinned
via `--tag v1.x.y`. Three frontends share one cache: CLI, Rust library,
MCP stdio server. Slim build ~3.7MB (`--no-default-features --features
mcp`); default build with semantic search ~13MB.

Shard layout is **content-addressed** on the git blob SHA of `xml/vk.xml`,
so the cache is byte-stable and portable across machines — pre-built
shards published by `.github/workflows/shards.yml` extract straight
into `$VKQUERY_CACHE_DIR`.

## Where to read next

| If you're doing… | Read |
|---|---|
| **Architecture overview** — data flow, design decisions, "why" | [`docs/architecture.md`](docs/architecture.md) |
| **CLI / library / MCP usage** | [`docs/usage.md`](docs/usage.md) |
| **Cache JSON schemas** | [`docs/data-model.md`](docs/data-model.md) |
| **Adding a query / index / search backend** | [`docs/extending.md`](docs/extending.md) |
| **Debugging a failure** | [`docs/troubleshooting.md`](docs/troubleshooting.md) |
| **User-facing quick reference** | `README.md` |
| **Checking implicit VUID derivation hasn't drifted** | diff `tests/fixtures/implicit_vuids_head.json` against a freshly built HEAD shard (recipe in `docs/usage.md` 6.4) |

## Project conventions

1. **Cache layout is the contract**. JSON files under
   `<root>/tags/<tag>/<vkxml_sha[:12]>/` must be byte-stable across
   machines. `serde_json::Map` is BTreeMap-backed by default, so emitting
   via `json!({...})` already gives sorted keys; `#[derive(Serialize)]`
   emits in field declaration order — when in doubt build via the
   `json!` macro.
2. **New indices must be added to `XML_INDEX_NAMES`** in
   `src/index/build.rs`. That's the freshness gate.
3. **Optional deps stay optional**. `embed` and `mcp` are cargo features
   and are both in the default set. For a slim ~3.7MB binary build with
   `--no-default-features --features mcp`. GPU backends (`cuda`, `cudnn`,
   `mkl`, `accelerate`, `metal`) are mutually exclusive and each implies
   `embed` — see `Cargo.toml` for the matrix.
4. **Implicit VUID derivation is a Rust reimplementation of upstream's
   `ValidityOutputGenerator`**. When upstream rules drift, fix the rule
   in `src/index/vuid_implicit.rs` and refresh
   `tests/fixtures/implicit_vuids_head.json` in the same PR.
5. **Tests assume a sibling `..\Vulkan-Docs` clone**. Integration tests
   walk up `CARGO_MANIFEST_DIR` looking for one.

## Quick sanity checks before claiming done

```powershell
$env:VKQUERY_CACHE_DIR = "C:\dev\vkquery-rs\target\cache"
cargo test --features mcp                                              # 46 tests, ~1m warm
cargo run --features mcp -- function vkCmdDraw --tag HEAD               # human summary (paged VUIDs)
cargo run --features mcp -- function vkCmdDraw --tag HEAD --json        # full payload incl. all VUIDs
cargo run --features mcp -- function vkCmdDraw --tag HEAD --all-vuids   # human, every VUID
cargo run --features mcp -- diff v1.3.250 v1.4.350 --entity features    # shows VK_VERSION_1_4 added
cargo run --features mcp -- search "image layout transition" --mode bm25 -k 3
cargo run --features mcp -- vuid VUID-vkCmdDraw-None-02691
```

CLI output is **human-readable by default** (a screenful summary with the
VUID list paged at 20). Pass `--json` for the full machine-readable
payload (the shape library/MCP consumers get), `--all-vuids` /
`--vuid-offset N` to page the VUID list in human mode.

For MCP smoke:

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  | (cat; sleep 3) | target/release/vkquery.exe mcp 2>/dev/null
```

Expected: 8 tools listed, all `vk_*` prefixed.

## File index

```
src/
  main.rs              bin entry — delegates to cli::run()
  lib.rs               pub mod re-exports
  api.rs               public query functions (entry point for library consumers)
  cli.rs               clap subcommands
  mcp_server.rs        rmcp stdio server (feature: mcp)
  types.rs             public data types (FunctionInfo, StructInfo, …)
  util.rs              type-name normalization, asciidoc markup stripper
  cache.rs             shard layout, content-hash, freshness checks
  docs_source.rs       Vulkan-Docs clone lifecycle
  git.rs               `git cat-file --batch` long-lived subprocess
  registry/
    mod.rs / schema.rs / parse.rs / legacy.rs    (roxmltree-based vk.xml parser)
  index/
    build.rs           orchestrator
    xml_index.rs       functions / structs / handles / enums / extensions / features / aliases
    reverse.rs         consumers + extended_by single-pass
    diff.rs            snapshot vs snapshot comparison
    prose.rs           chapter section splitter (BM25 corpus source)
    vuid_explicit.rs   regex over chapters/*.adoc with ifdef + commonvalidity
    vuid_implicit.rs   re-derives ValidityOutputGenerator rules from XML attrs
  search/
    bm25.rs            from-scratch Okapi BM25 (k1=1.5, b=0.75, eps=0.25)
    hybrid.rs          RRF fusion of BM25 + embedding top-k (feature: embed)
    embedding.rs       candle 0.8 + bge-small-en-v1.5 (feature: embed)
tests/
  integration.rs       end-to-end against sibling Vulkan-Docs clone
  parity_bm25.rs       BM25 top-5 fixture diff (tests/fixtures/bm25_top5_head.json)
  embeddings_sanity.rs cosine-similarity sanity for bge-small
  fixtures/            golden implicit VUID dump + BM25 top-5 fixture
docs/
  usage.md             detailed CLI / library / MCP usage manual
.github/workflows/
  ci.yml               3-OS test matrix + embed-build smoke
  release.yml          v* tag → 3-target binaries + shards (calls shards.yml)
  shards.yml           HEAD + latest 5 v1.x.y tags → slim shard tarballs; weekly schedule + workflow_dispatch + workflow_call
```

## Feature checklist

All major features are implemented and tested. Use this as a quick
reference for what's available and where the trade-offs live.

| Feature | State |
|---|---|
| Registry parser + 5 XML queries (function / struct / extensions / callers / deps) | ✓ — `src/registry/`, `src/index/xml_index.rs`, `src/index/reverse.rs` |
| Version diff (`diff v1 v2 --entity …`) | ✓ — `src/index/diff.rs` |
| Explicit VUIDs (chapters/*.adoc with `ifdef::` + `commonvalidity` expansion) | ✓ — 19,833 entries on HEAD; `src/index/vuid_explicit.rs` |
| Implicit VUIDs (re-derived from XML attrs) | ✓ — 6,573/6,575 IDs; 2 char[N] static-length rules still TODO; `src/index/vuid_implicit.rs` |
| BM25 search | ✓ — pure-Rust Okapi (k1=1.5, b=0.75, eps=0.25); fixture diff in `tests/parity_bm25.rs` |
| Semantic embeddings | ✓ — candle 0.8 + bge-small-en-v1.5; CPU is slow (~32 vec/30s), set `VKQUERY_EMBED_LIMIT=N` for fast iteration; GPU backends via `cuda`/`metal`/`mkl` features |
| Hybrid search (RRF fusion) | ✓ — `src/search/hybrid.rs`, c=60; falls back to BM25-only if embeddings unavailable |
| MCP server | ✓ — 8 `vk_*` tools, stdio transport; `vkquery mcp` |
| CI (3-OS test matrix + linux embed build) | ✓ — `.github/workflows/ci.yml` |
| Release pipeline (3-target binaries + sha256 + GitHub Release on `v*`) | ✓ — `.github/workflows/release.yml` |
| Pre-built shard distribution (HEAD + latest 5 v1.x.y slim shards, weekly + per-v*) | ✓ — `.github/workflows/shards.yml`; `vkquery cache info` for local introspection |
