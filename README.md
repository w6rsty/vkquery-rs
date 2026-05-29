# vkquery (Rust)

[![CI](https://github.com/w6rsty/vkquery-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/w6rsty/vkquery-rs/actions/workflows/ci.yml)
[![Release](https://github.com/w6rsty/vkquery-rs/actions/workflows/release.yml/badge.svg)](https://github.com/w6rsty/vkquery-rs/actions/workflows/release.yml)
[![Pre-built shards](https://github.com/w6rsty/vkquery-rs/actions/workflows/shards.yml/badge.svg)](https://github.com/w6rsty/vkquery-rs/actions/workflows/shards.yml)

English · [简体中文](docs/README.zh-CN.md)

Fast, version-pinned query layer over [Khronos Vulkan-Docs](https://github.com/KhronosGroup/Vulkan-Docs).
Ask the Vulkan spec questions like "what are the VUIDs for `vkCmdDraw`?",
"which structs extend `VkImageCreateInfo`'s pNext chain?", or "what
changed between v1.3.250 and v1.4.350?" — get JSON back in milliseconds.

Single static binary (~3.7MB slim, ~13MB with semantic search). No
Python, no Ruby, no `asciidoctor` required at query time.

## Why vkquery?

The Vulkan-Docs repo (vk.xml + asciidoc chapters) is the canonical
source for the spec, but it's built to render into a human-readable
PDF/HTML, not to be queried programmatically. Several common questions
need a full Ruby + Asciidoctor (+ Python `ValidityOutputGenerator`)
spec build to answer, or careful hand-grepping. vkquery turns each
into a one-liner:

| Question | Without vkquery | With vkquery |
|---|---|---|
| "What are the VUIDs for `vkCmdDraw` at v1.4.352?" | Clone Vulkan-Docs, install Ruby + Asciidoctor + the `vu-to-json` extension, run a full spec build per tag (~minutes), parse `validusage.json`. Or grep `chapters/*.adoc` by hand while tracking `ifdef::EXT[]` guards mentally. | `vkquery function vkCmdDraw --tag v1.4.352` |
| "Which structs extend `VkImageCreateInfo`'s pNext chain?" | Scan every `<type structextends="...">` in vk.xml yourself — not surfaced by any upstream tool. | included in `vkquery struct VkImageCreateInfo` |
| "Which commands consume `VkImage`?" | Manual XML sweep over every `<param>` / `<member>`. | `vkquery callers VkImage` |
| "What changed between v1.3.250 and v1.4.350?" | Two git checkouts, two full spec parses, hand-diff. | `vkquery diff v1.3.250 v1.4.350 --entity features` |
| "Find the section about image layout transitions, by meaning not just keywords." | No upstream tool — roll your own BM25 / embedding pipeline over ~85 asciidoc files. | `vkquery search "image layout transition" --mode hybrid` |
| "Wire spec lookup into an LLM agent." | Hand-roll JSON wrappers around the above, manage caches, strip asciidoc markup. | `vkquery mcp` — 8 typed MCP tools, drop-in for Claude Code / Cursor / any MCP client. |

**Implicit VUIDs are the headline difference.** Upstream's
`ValidityOutputGenerator` emits ~6,500 parameter-validity / parent-handle
/ command-pool constraints during the asciidoctor spec build — they
don't exist as a queryable artifact otherwise. vkquery re-derives them
in pure Rust at index time, so the full set ships alongside the
explicit VUIDs from your first query, with no spec build toolchain on
your machine.

Net effect: spec lookups go from "set up a multi-language build
environment" to "download a ~5MB binary".

## Features

- **8 query primitives** for functions, structs, extensions, callers,
  dependencies, VUIDs, version diffs, and prose search
- **Version-pinned** — every query takes `--tag v1.x.y` so answers track
  the spec revision you care about
- **Three frontends** — CLI, Rust library (`use vkquery::api::*;`), and
  MCP stdio server (8 tools, drop-in for any MCP client)
- **Three search modes** — lexical BM25, semantic BERT embeddings, and
  hybrid RRF fusion
- **Pre-built shards on GitHub Releases** — skip the first-query
  indexing cost; one tarball per recent v1.x.y tag

| Query | Answers |
|---|---|
| `function <name>` | signature, params, queues, renderpass, VUIDs, version availability |
| `struct <name>` | members, structextends, extended_by, VUIDs |
| `extensions [--type --author --status]` | filtered extension list |
| `diff <v1> <v2> [--entity …]` | added / removed / changed / promoted between tags |
| `callers <type>` | every command/struct that consumes this type |
| `deps <function>` | parent handle chain, required exts/features, pNext, externsync, queue/renderpass |
| `vuid <id>` | rule text + source file + guard extensions |
| `search <query> [--mode bm25\|embed\|hybrid]` | BM25 / semantic / hybrid over prose + VUID text |

Commands print a **human-readable summary by default** — a screenful,
with long VUID lists paged (20 at a time; `--all-vuids` or `--vuid-offset
N` to page, `--limit` for extension lists). Add `--json` to any query for
the full machine-readable payload.

## Install

```bash
cargo install --path .                                       # default — mcp + embed (~13MB)
cargo install --path . --no-default-features --features mcp  # slim (~5.4MB), no semantic search
cargo install --path . --no-default-features                 # library only, no MCP, no embed
```

Or grab a pre-built `mcp`-only binary from
[GitHub Releases](https://github.com/w6rsty/vkquery-rs/releases) for
Linux x86-64, macOS arm64, or Windows x86-64. GPU backends (CUDA, Metal,
MKL) require `cargo install` from source.

Different builds ship different capabilities — the release binary is
slim (`mcp`, no `embed`), so it offers `search --mode bm25` only and
hides the `embed`/`hybrid` modes. Run `vkquery --version` or `vkquery
config` to see which features your binary was built with; the CLI only
exposes flags and subcommands the build actually supports.

For development:

```bash
cargo build --release --features embed,mcp
```

On first `search --mode embed` call (or first `index build` with the
`embed` feature on), the binary downloads `BAAI/bge-small-en-v1.5`
(~130MB) into `<dirs::cache_dir>/vkquery/models/BAAI--bge-small-en-v1.5/`.

The binary auto-clones the Vulkan-Docs repo on first run (into
`%LOCALAPPDATA%\vkquery\Vulkan-Docs\` on Windows or
`$XDG_CACHE_HOME/vkquery/Vulkan-Docs/` elsewhere). Override the clone
location with `VKQUERY_DOCS_PATH=<path>`. Override the cache with
`VKQUERY_CACHE_DIR=<path>`.

Run `vkquery config` to see the resolved cache dir / docs path, the
current values of every recognised `VKQUERY_*` env var, and which cargo
features the binary was built with. Add `--json` for machine-readable
output.

### Pre-built shards (optional)

Each `function` / `struct` / … call lazily builds the shard for the
requested tag on first use (~minutes per tag, including the BM25 corpus
build). To skip that one-time cost, fetch a pre-built slim shard:

```bash
vkquery index fetch --tag v1.4.352   # download + verify + extract one tag
vkquery index fetch                  # defaults to --tag HEAD
vkquery index fetch --all            # every tag published on the release
```

`index fetch` downloads the tarball from GitHub Releases, checks it
against the published SHA-256, extracts it into your `$VKQUERY_CACHE_DIR`,
and registers it — no `curl`/`tar` and no Vulkan-Docs clone required for
the fetch itself. It is idempotent (re-running skips an already-present
shard; pass `--force` to re-download), and `--release <tag>` lets you pin
to a specific `v*` release instead of the rolling one.

Pre-built shards live on the rolling
[`shards-latest`](https://github.com/w6rsty/vkquery-rs/releases/tag/shards-latest)
release (refreshed weekly with HEAD + the 5 most recent `v1.x.y` tags)
and also appear as assets on each `v*` binary release. Tarballs are
slim — they contain the BM25 + XML indices but **not** the BERT
embeddings layer. To use `--mode embed` / `--mode hybrid`, run
`vkquery index build --tag <T> --force` once after fetching so the
local embedding pass runs alongside the already-extracted shard.

## GPU acceleration

CPU BERT inference on Windows without MKL clocks ~32 vec/30s, so a full
HEAD embed (~27K texts) takes ~7h. For production use, build with one
of these mutually-exclusive cargo features — each one implies `embed`,
so you do **not** also need to pass `--features embed`:

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

Each `function` / `struct` / `extensions` / etc. call lazily builds the
shard for the requested tag if it isn't already in the cache. Cold
build for HEAD is ~4 seconds; warm queries are <50ms.

## MCP server

```bash
vkquery mcp                # stdio transport; usable from any MCP client
```

The 8 tools expose the same shapes as the CLI (`vk_get_function`,
`vk_get_struct`, `vk_list_extensions`, `vk_diff_versions`, `vk_find_callers`,
`vk_find_dependencies`, `vk_get_vuid`, `vk_search_concept`). Configure your
client to launch `vkquery mcp` as the command.

## Known gaps

- **Embedding throughput**: CPU BERT on Windows without MKL is ~32 vec/30s,
  so a full HEAD embed (~27K texts) takes ~7h. For dev iteration, set
  `VKQUERY_EMBED_LIMIT=N` to cap the corpus, or `VKQUERY_SKIP_EMBED=1`
  to skip embeddings during shard build (BM25 still indexes). Production
  needs `--features cuda` / `--features mkl` (not enabled by default).

For detailed usage (CLI / library / MCP / configuration), see
[docs/usage.md](docs/usage.md).

## License

Apache-2.0
