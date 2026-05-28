# CLAUDE.md — vkquery (Rust)

Agent entrypoint for the Rust port of `vkquery`. Read this first.

## What this repo is

`vkquery-rs` is a Rust rewrite of the Python `vkquery` package
(reference impl at `../vkquery/`). Same 8 query primitives, same shard
layout, same Vulkan-Docs git-tag pinning. Goal: single static binary
(currently ~2.5MB stripped on Windows x86-64 without the `embed`
feature; ~35MB with). Both impls write to the same cache so the Rust
build can read Python-built shards and vice versa.

## Where to read next

| If you're doing… | Read |
|---|---|
| **Understanding the port** — what's done, what's deferred, why | this file + git log |
| **CLI / library / MCP usage** (detailed) | [`docs/usage.md`](docs/usage.md) |
| **Quick reference** | `README.md` |
| **Hitting parity gaps** | look at `tests/fixtures/implicit_vuids_head.json` and compare via the Python shard at `%LOCALAPPDATA%\vkquery\tags\HEAD\<sha>\` |
| **Adding a new query / index** | mirror the matching Python module in `../vkquery/src/vkquery/` |

## Project conventions

1. **Cache layout is the contract**. JSON files under
   `<root>/tags/<tag>/<vkxml_sha[:12]>/` must stay byte-stable with
   Python's `json.dumps(..., indent=2, sort_keys=True)`. `serde_json::Map`
   is BTreeMap-backed by default, so emitting via `json!({...})` already
   gives sorted keys; struct-derive `Serialize` emits in field declaration
   order — when in doubt build via the `json!` macro.
2. **New indices must be added to `XML_INDEX_NAMES`** in
   `src/index/build.rs`. That's the freshness gate.
3. **Optional deps stay optional**. `embed` and `mcp` are cargo features.
   `embed` is excluded from default features pending the candle 0.8 bump.
4. **Don't fork Vulkan-Docs parsing**. The python ref reuses `reg.py` /
   `validitygenerator.py`. The Rust port re-derives — when implicit VUID
   rules drift upstream, fix the rule in `src/index/vuid_implicit.rs` and
   refresh `tests/fixtures/implicit_vuids_head.json` in the same PR.
5. **Tests assume a sibling `..\Vulkan-Docs` clone**. Integration tests
   walk up `CARGO_MANIFEST_DIR` looking for one.

## Quick sanity checks before claiming done

```powershell
$env:VKQUERY_CACHE_DIR = "C:\dev\vkquery-rs\target\cache"
cargo test --features mcp                                              # 31 tests, ~1m warm
cargo run --features mcp -- function vkCmdDraw --tag HEAD               # full payload incl. VUIDs
cargo run --features mcp -- diff v1.3.250 v1.4.350 --entity features    # shows VK_VERSION_1_4 added
cargo run --features mcp -- search "image layout transition" --mode bm25 -k 3
cargo run --features mcp -- vuid VUID-vkCmdDraw-None-02691
```

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
    hybrid.rs / embedding.rs    (deferred — feature: embed)
tests/
  integration.rs       end-to-end against sibling Vulkan-Docs clone
  parity_bm25.rs       R6 BM25 top-5 parity vs Python fixture
  embeddings_sanity.rs cosine-similarity sanity for bge-small
  fixtures/            golden implicit VUID dump + Python BM25 top-5 fixture
docs/
  usage.md             detailed CLI / library / MCP usage manual
```

## Status (R0–R8 of the rewrite plan)

| R | Done | Notes |
|---|---|---|
| R0–R3 | ✓ | scaffold, registry parser, XML index + 5 queries, tag/diff |
| R4 explicit VUIDs | ✓ | 100% id, 100% text vs Python |
| R5 implicit VUIDs | ✓ (100.00% recall / 100.00% precision, 6575/6575) | text parity 92.1% — remaining drift is asciidoc markup / whitespace micro-differences |
| R6 BM25 search | ✓ | `tests/parity_bm25.rs` passes; fixture in `tests/fixtures/bm25_top5_head.json` (regen via `target/dump_bm25_top5.py`) |
| R7 MCP server | ✓ | 8 tools registered, init+tools/list+tools/call verified |
| R7 candle embeddings | ✓ | candle 0.8 + bge-small; CPU is the bottleneck (`VKQUERY_EMBED_LIMIT` for fast iteration) |
| R8 polish | partial | README + CLAUDE.md + docs/usage.md done; CI cross-compile + GH release TODO |
