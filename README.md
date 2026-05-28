# vkquery (Rust)

Query/retrieval layer over [Khronos Vulkan-Docs](https://github.com/KhronosGroup/Vulkan-Docs).
Single static binary (~2.5MB stripped on Windows x86-64). No Python, no Ruby,
no `asciidoctor` required to query the spec.

This is a Rust port of the Python `vkquery` reference (lives at
`../vkquery/`). Both implementations write to the same content-addressed
shard layout so they can read each other's caches.

## What it gives you

8 query primitives, all version-pinned via `--tag <v1.x.y>`:

| Query | Answers |
|---|---|
| `function <name>` | signature, params, queues, renderpass, VUIDs, version availability |
| `struct <name>` | members, structextends, extended_by, VUIDs |
| `extensions [--type --author --status]` | filtered extension list |
| `diff <v1> <v2> [--entity …]` | added / removed / changed / promoted between tags |
| `callers <type>` | every command/struct that consumes this type |
| `deps <function>` | parent handle chain, required exts/features, pNext, externsync, queue/renderpass |
| `vuid <id>` | the rule text + source file + guard extensions |
| `search <query> [--mode bm25\|embed\|hybrid]` | BM25 over prose + VUID text (embed/hybrid land with R7) |

Three frontends share that surface: the CLI, the library (`use vkquery::api::*;`),
and the MCP stdio server (`vkquery mcp` exposes the same 8 calls as MCP tools).

## Install

```bash
cargo install --path .                       # default features = mcp + embed (~8MB binary)
cargo install --path . --features mcp        # without embed (~2.5MB skinny build)
cargo install --path . --no-default-features # library / CLI only, no MCP, no embed
```

Or grab the prebuilt `mcp`-only binary for your platform from
[GitHub Releases](https://github.com/w6rsty/vkquery-rs/releases) (Linux x86-64,
macOS arm64, Windows x86-64; GPU backends still need `cargo install` from
source). For development:

```bash
cargo build --release --features embed,mcp
```

On first `search --mode embed` call (or first `index build` with `embed`
feature on), the binary downloads `BAAI/bge-small-en-v1.5` (~130MB) into
`<dirs::cache_dir>/vkquery/models/BAAI--bge-small-en-v1.5/`.

The binary auto-clones the Vulkan-Docs repo on first run (into `%LOCALAPPDATA%\vkquery\Vulkan-Docs\`
on Windows or `$XDG_CACHE_HOME/vkquery/Vulkan-Docs/` elsewhere). Override the
clone location with `VKQUERY_DOCS_PATH=<path>`. Override the cache with
`VKQUERY_CACHE_DIR=<path>`.

### Pre-built shards (optional)

Each `function` / `struct` / … call lazily builds the shard for the requested
tag on first use (~minutes per tag, including the BM25 corpus build). To skip
that one-time cost, drop a pre-built slim shard into your cache:

```bash
# Find where vkquery wants its cache to live:
vkquery cache info

# Then, for the tag you care about:
curl -L -o vkquery-shard-v1.4.352-slim.tar.gz \
  https://github.com/w6rsty/vkquery-rs/releases/download/shards-latest/vkquery-shard-v1.4.352-slim.tar.gz
tar -xzf vkquery-shard-v1.4.352-slim.tar.gz -C "$VKQUERY_CACHE_DIR"
```

Pre-built shards live on the rolling [`shards-latest`](https://github.com/w6rsty/vkquery-rs/releases/tag/shards-latest)
release (refreshed weekly with HEAD + the 5 most recent `v1.x.y` tags) and
also appear as assets on each `v*` binary release. Tarballs are slim — they
contain the BM25 + XML indices but **not** the BERT embeddings layer. To use
`--mode embed` / `--mode hybrid`, run `vkquery index build --tag <T> --force`
once after extraction so the local embedding pass runs alongside the
already-extracted shard.

Windows users: `tar.exe` ships with Windows 10+; the recipe above works
unchanged in PowerShell.

## GPU acceleration

CPU BERT inference on Windows without MKL clocks ~32 vec/30s, so a full HEAD
embed (~27K texts) takes ~7h. For production use, build with one of these
mutually-exclusive cargo features — each one implies `embed`, so you do
**not** also need to pass `--features embed`:

| Feature | Backend | Platform | Notes |
|---|---|---|---|
| `cuda` | NVIDIA CUDA | Linux / Windows | Needs CUDA Toolkit ≥ 12 on `PATH` |
| `cudnn` | CUDA + cuDNN | Linux / Windows | Implies `cuda`; needs cuDNN libs on `PATH` |
| `mkl` | Intel MKL | Linux / Windows (x86_64) | Needs Intel oneAPI; `LD_LIBRARY_PATH` / `PATH` set |
| `accelerate` | Apple Accelerate | macOS | No setup needed |
| `metal` | Apple GPU | macOS | No setup needed |

```bash
cargo build --release --features cuda                 # CUDA only (also pulls embed)
cargo build --release --features cudnn                # cuDNN (transitively cuda + embed)
cargo build --release --features mkl                  # Intel MKL on Linux/Windows
cargo build --release --features metal                # macOS GPU
```

**Don't combine backends.** Enabling more than one (`--features "cuda mkl"`)
will fail at candle's build step with an unfriendly error. Pick one.

## Examples

```bash
vkquery function vkCmdDraw --tag v1.4.352
vkquery struct VkImageCreateInfo
vkquery extensions --type device --author KHR --status active
vkquery diff v1.3.250 v1.4.350 --entity features    # shows VK_VERSION_1_4 added
vkquery callers VkImage
vkquery deps vkCmdDraw
vkquery vuid VUID-vkCmdDraw-None-02691
vkquery search "image layout transition" --mode bm25 -k 3
```

Each `function` / `struct` / `extensions` / etc. call lazily builds the shard
for the requested tag if it isn't already in the cache. Cold build for HEAD
is ~4 seconds; warm queries are <50ms.

## MCP server

```bash
vkquery mcp                # stdio transport; usable from any MCP client
```

The 8 tools expose the same shapes as the CLI (`vk_get_function`,
`vk_get_struct`, `vk_list_extensions`, `vk_diff_versions`, `vk_find_callers`,
`vk_find_dependencies`, `vk_get_vuid`, `vk_search_concept`). Configure your
client to launch `vkquery mcp` as the command.

## Status (parity vs Python reference)

| Indices | Parity vs `../vkquery/` |
|---|---|
| functions / structs / handles / enums / extensions / features / aliases | 100% id, 99.4% byte |
| explicit VUIDs | 100% id, 100% text (19,833 entries) |
| implicit VUIDs | **100.00% recall, 100.00% precision** (6575/6575); 92.1% text |
| BM25 search | parity test `tests/parity_bm25.rs` passes (≥2/5 overlap + ≤5% top-1 score drift) |
| semantic embeddings (`--mode embed`) | candle 0.8 + bge-small-en-v1.5; 5-sentence sanity ✓ |
| hybrid search (`--mode hybrid`) | RRF fusion of bm25 + embed lists ✓ |

For detailed usage (CLI / library / MCP / configuration / CI patterns),
see [docs/usage.md](docs/usage.md).

## Known gaps

- **Embedding throughput**: CPU BERT on Windows without MKL is ~32 vec/30s,
  so a full HEAD embed (~27K texts) takes ~7h. For dev iteration, set
  `VKQUERY_EMBED_LIMIT=N` to cap the corpus, or `VKQUERY_SKIP_EMBED=1`
  to skip embeddings during shard build (BM25 still indexes). Production
  needs `--features cuda` / `--features mkl` (not enabled by default).
- **R5 implicit VUIDs text drift**: 7.9% of the 6,575 common VUIDs differ from
  Python by whitespace / asciidoc markup micro-differences. IDs match exactly.

## License

Apache-2.0
