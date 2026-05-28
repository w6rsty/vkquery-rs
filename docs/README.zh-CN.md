# vkquery (Rust)

[![CI](https://github.com/w6rsty/vkquery-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/w6rsty/vkquery-rs/actions/workflows/ci.yml)
[![Release](https://github.com/w6rsty/vkquery-rs/actions/workflows/release.yml/badge.svg)](https://github.com/w6rsty/vkquery-rs/actions/workflows/release.yml)
[![Pre-built shards](https://github.com/w6rsty/vkquery-rs/actions/workflows/shards.yml/badge.svg)](https://github.com/w6rsty/vkquery-rs/actions/workflows/shards.yml)

[English](../README.md) · 简体中文

[Khronos Vulkan-Docs](https://github.com/KhronosGroup/Vulkan-Docs) 的
快速、版本锁定查询层。可以这样问 Vulkan 规范：「`vkCmdDraw` 有哪些
VUID？」、「`VkImageCreateInfo` 的 pNext 链上能挂哪些 struct？」、
「v1.3.250 到 v1.4.350 之间改了什么？」——毫秒级拿到 JSON。

单一静态二进制（slim ~3.7MB，含语义搜索默认 ~13MB）。查询路径上
不依赖 Python / Ruby / `asciidoctor`。

## 为什么用 vkquery？

Vulkan-Docs 仓库（vk.xml + asciidoc 章节）是 spec 的权威源，但设计
目标是渲染成给人读的 PDF/HTML，不是给程序检索的。几类常见问题要么
得跑完整的 Ruby + Asciidoctor（+ Python `ValidityOutputGenerator`）
spec build，要么得仔细手撕原始文件。vkquery 把每一个都变成一行命令：

| 想问的 | 没有 vkquery 怎么办 | 用 vkquery |
|---|---|---|
| 「`vkCmdDraw` 在 v1.4.352 的 VUID？」 | 克隆 Vulkan-Docs，装 Ruby + Asciidoctor + `vu-to-json` 扩展，每个 tag 跑一次完整 spec build（数分钟），然后解析 `validusage.json`。或者手 grep `chapters/*.adoc`，脑子里维护 `ifdef::EXT[]` 守卫栈。 | `vkquery function vkCmdDraw --tag v1.4.352` |
| 「哪些 struct 能扩展 `VkImageCreateInfo` 的 pNext 链？」 | 自己扫 vk.xml 里所有 `<type structextends="...">`——上游工具不暴露这个反向索引。 | `vkquery struct VkImageCreateInfo` 返回里就有 |
| 「哪些命令消费 `VkImage`？」 | 手动扫所有 `<param>` / `<member>`。 | `vkquery callers VkImage` |
| 「v1.3.250 → v1.4.350 之间改了什么？」 | 两次 git checkout，两次完整 spec 解析，手动 diff。 | `vkquery diff v1.3.250 v1.4.350 --entity features` |
| 「按语义而不是关键字找『image layout transition』相关章节」 | 上游没有这类工具——自己拉一套 BM25/embedding pipeline 覆盖 85 个 asciidoc 文件。 | `vkquery search "image layout transition" --mode hybrid` |
| 「把 spec 查询接进 LLM agent」 | 手撕 JSON 包装、管缓存、剥 asciidoc 标记。 | `vkquery mcp`——8 个类型化 MCP 工具，开箱接 Claude Code / Cursor / 任何 MCP 客户端。 |

**Implicit VUIDs 是最大的差异点。** 上游的 `ValidityOutputGenerator`
在 asciidoctor spec build 期间产出约 6500 条参数有效性 / 父句柄关系 /
command pool 限制规则——不跑完整 spec build 这些规则根本不是可查询
的产物。vkquery 用纯 Rust 在 index 构建时重新派生 6573 条 ID（还差
2 条 char[N] 静态数组规则待补），所以从第一次查询起，implicit 与
explicit VUID 都在一份 JSON 里返回，本机不需要任何 spec build 工具链。

净效果：spec 检索从「先搭一套多语言构建环境」降到「下载一个 4MB
二进制」。

## 功能特性

- **8 类查询原语** —— functions、structs、extensions、callers、deps、
  VUIDs、version diff、prose search
- **版本锁定** —— 所有命令接受 `--tag v1.x.y`，回答永远与你关心的
  spec 修订对应
- **三套前端** —— CLI、Rust 库（`use vkquery::api::*;`）、MCP stdio
  服务（8 个工具，可直接接任意 MCP 客户端）
- **三种搜索模式** —— 词法 BM25、语义 BERT embedding、混合 RRF 融合
- **GitHub Releases 上的预构建 shard** —— 跳过首次查询的索引开销；
  每个最近 v1.x.y tag 一份 tarball

| 查询 | 返回 |
|---|---|
| `function <name>` | 签名、参数、queue/renderpass 限制、VUIDs、可用版本 |
| `struct <name>` | 成员、`structextends`、`extended_by`、VUIDs |
| `extensions [--type --author --status]` | 过滤后的扩展列表 |
| `diff <v1> <v2> [--entity …]` | 两个 tag 之间新增/删除/变更/晋升 |
| `callers <type>` | 消费此类型的所有命令/结构 |
| `deps <function>` | 父句柄链、依赖扩展/feature、pNext、externsync、queue/renderpass |
| `vuid <id>` | 规则文本 + 来源文件 + 守卫扩展 |
| `search <query> [--mode bm25\|embed\|hybrid]` | 在 prose + VUID 文本上做 BM25 / 语义 / 混合搜索 |

## 安装

```bash
cargo install --path .                                       # 默认 — mcp + embed (~13MB)
cargo install --path . --no-default-features --features mcp  # slim (~3.7MB)，不含语义搜索
cargo install --path . --no-default-features                 # 仅库，不含 MCP 与 embed
```

或者从 [GitHub Releases](https://github.com/w6rsty/vkquery-rs/releases)
直接抓预编译的 `mcp`-only 二进制，覆盖 Linux x86-64 / macOS arm64 /
Windows x86-64。GPU 后端（CUDA、Metal、MKL）需要 `cargo install` 从源
码编译。

开发用：

```bash
cargo build --release --features embed,mcp
```

第一次跑 `search --mode embed`（或第一次带 `embed` 跑 `index build`）
时，二进制会从 HuggingFace 下载 `BAAI/bge-small-en-v1.5`（~130MB）
到 `<dirs::cache_dir>/vkquery/models/BAAI--bge-small-en-v1.5/`。

二进制第一次启动会自动克隆 Vulkan-Docs（Windows 上克到
`%LOCALAPPDATA%\vkquery\Vulkan-Docs\`，其他平台
`$XDG_CACHE_HOME/vkquery/Vulkan-Docs/`）。用 `VKQUERY_DOCS_PATH=<path>`
覆盖克隆位置；用 `VKQUERY_CACHE_DIR=<path>` 覆盖缓存位置。

### 预构建 shard（可选）

每次 `function` / `struct` / … 查询都会按需懒构建对应 tag 的 shard
（含 BM25 语料，需要数分钟）。要跳过这一次性开销，可以直接把预构建
的 slim shard 解压到本地缓存：

```bash
# 找出 vkquery 想用的 cache 路径
vkquery cache info

# 然后下载你关心的 tag
curl -L -o vkquery-shard-v1.4.352-slim.tar.gz \
  https://github.com/w6rsty/vkquery-rs/releases/download/shards-latest/vkquery-shard-v1.4.352-slim.tar.gz
tar -xzf vkquery-shard-v1.4.352-slim.tar.gz -C "$VKQUERY_CACHE_DIR"
```

预构建 shard 由滚动 release
[`shards-latest`](https://github.com/w6rsty/vkquery-rs/releases/tag/shards-latest)
托管（每周一 06:00 UTC 刷新，覆盖 HEAD + 最近 5 个 v1.x.y tag），同时
也出现在每个 `v*` 二进制 release 的资产里。tarball 是 **slim** 版——
只含 BM25 + XML 索引，**不含** BERT embedding 层。要用 `--mode embed`
/ `--mode hybrid`，解压后跑一次
`vkquery index build --tag <T> --force` 让本地补一遍 embedding。

Windows 用户：`tar.exe` 在 Win10+ 内置；上述命令在 PowerShell 同样可用。

## GPU 加速

Windows 上无 MKL 时 CPU BERT 推理约 32 vec/30s，全量 HEAD embedding
要 ~7 小时。生产场景启用以下**互斥**的 cargo feature 之一——每个都
隐含 `embed`，不必额外加 `--features embed`：

| Feature | 后端 | 平台 | 说明 |
|---|---|---|---|
| `cuda` | NVIDIA CUDA | Linux / Windows | CUDA Toolkit ≥ 12 要在 `PATH` 上 |
| `cudnn` | CUDA + cuDNN | Linux / Windows | 隐含 `cuda`；cuDNN 库要在 `PATH` 上 |
| `mkl` | Intel MKL | Linux / Windows (x86_64) | 装 Intel oneAPI；设好 `LD_LIBRARY_PATH` / `PATH` |
| `accelerate` | Apple Accelerate | macOS | 无需配置 |
| `metal` | Apple GPU | macOS | 无需配置 |

```bash
cargo build --release --features cuda                 # 仅 CUDA（顺带 embed）
cargo build --release --features cudnn                # cuDNN（透传 cuda + embed）
cargo build --release --features mkl                  # Intel MKL on Linux/Windows
cargo build --release --features metal                # macOS GPU
```

**不要混用后端。** 同时启用多个（如 `--features "cuda mkl"`）会在
candle 编译时报不友好的错。选一个就好。

## 示例

```bash
vkquery function vkCmdDraw --tag v1.4.352
vkquery struct VkImageCreateInfo
vkquery extensions --type device --author KHR --status active
vkquery diff v1.3.250 v1.4.350 --entity features    # 显示 VK_VERSION_1_4 新增
vkquery callers VkImage
vkquery deps vkCmdDraw
vkquery vuid VUID-vkCmdDraw-None-02691
vkquery search "image layout transition" --mode bm25 -k 3
```

每次 `function` / `struct` / `extensions` 等调用，缓存中不存在该 tag
的 shard 时会按需懒构建。HEAD 冷构建约 4 秒；命中缓存的查询 <50ms。

## MCP 服务

```bash
vkquery mcp                # stdio 传输，可被任何 MCP 客户端启动
```

8 个工具与 CLI 一一对应（`vk_get_function`、`vk_get_struct`、
`vk_list_extensions`、`vk_diff_versions`、`vk_find_callers`、
`vk_find_dependencies`、`vk_get_vuid`、`vk_search_concept`）。MCP 客户
端里把启动命令配为 `vkquery mcp` 即可。

## 已知限制

- **CPU embedding 吞吐量低**：Windows 上无 MKL 时 BERT 约 32 vec/30s，
  全量 HEAD（~27K 条文本）需 ~7 小时。开发时设
  `VKQUERY_EMBED_LIMIT=N` 限制语料规模，或 `VKQUERY_SKIP_EMBED=1`
  完全跳过 embedding 构建（BM25 仍建）。生产用 `--features cuda` /
  `--features mkl`（默认 features 未启用 GPU 后端）。

详细使用文档（CLI / 库 / MCP / 配置）请见 [docs/usage.md](usage.md)。

## License

Apache-2.0
