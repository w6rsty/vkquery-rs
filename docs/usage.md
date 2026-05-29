# 使用手册

`vkquery-rs` 是 Khronos Vulkan-Docs 的查询层，单二进制即可运行。三套前端
共享同一份缓存：CLI、Rust 库、MCP stdio 服务。本文档覆盖所有可用功能与
具体使用方法。

## 一、构建与安装

### 1.1 从源码构建

```bash
cd C:\dev\vkquery-rs

# 默认 = mcp + embed（约 13MB，含 BM25 / 语义搜索 / MCP 服务）
cargo build --release

# Slim：仅 BM25 + MCP（约 3.7MB，不含 candle BERT 嵌入）
cargo build --release --no-default-features --features mcp

# 最小：仅库 / CLI，不含 MCP 与嵌入
cargo build --release --no-default-features
```

可选 cargo features：

| Feature | 作用 | 体积影响 |
|---|---|---|
| `mcp` | 启用 `vkquery mcp` 命令；引入 `rmcp 0.2` + `tokio` | +约 1MB |
| `embed` | 启用语义搜索；引入 `candle-core 0.8` + `tokenizers` + bge-small-en-v1.5 | +约 9MB（不含模型权重） |

二进制位于 `target/release/vkquery.exe`（Windows）或 `target/release/vkquery`。

### 1.2 用 cargo install 安装

```bash
cargo install --path .
# 或指定特性
cargo install --path . --features mcp --no-default-features
```

### 1.3 直接下载预编译二进制

每个 `v*` tag 会触发 `.github/workflows/release.yml` 在三个平台
（`x86_64-unknown-linux-gnu`、`aarch64-apple-darwin`、`x86_64-pc-windows-msvc`）
构建仅含 `mcp` 特性的精简二进制（约 3.7MB），并连同 SHA256 校验上传到
[GitHub Releases](https://github.com/w6rsty/vkquery-rs/releases)。需要 GPU
后端（`cuda` / `metal` / `mkl`）的用户仍需自己 `cargo install` 从源码编译。

## 二、配置

### 2.1 环境变量

| 变量 | 默认值 | 用途 |
|---|---|---|
| `VKQUERY_DOCS_PATH` | 同级 `Vulkan-Docs/` 目录，否则 `<cache>/Vulkan-Docs` | Vulkan-Docs 克隆路径 |
| `VKQUERY_CACHE_DIR` | `%LOCALAPPDATA%\vkquery`（Win）/ `~/.cache/vkquery`（其他） | shard 缓存位置 |
| `VKQUERY_SKIP_EMBED` | （未设） | 设为 `1` 时跳过 embedding 索引构建（BM25 仍建） |
| `VKQUERY_EMBED_LIMIT` | （未设） | 限制 embedding 语料的前 N 条，开发用 |
| `VKQUERY_HYBRID_RRF_K` | `60` | hybrid 搜索的 RRF 阻尼常数；较小值放大 top-rank 差距，用于调参实验 |
| `VKQUERY_NO_PROGRESS` | （未设） | 设为任何值时禁用 stderr 上的进度条（spinner + embedding 进度条）；管道 / MCP / 非 TTY 环境自动禁用 |

### 2.2 首次运行行为

第一次查询任何 tag 会触发以下步骤：

1. 若 `VKQUERY_DOCS_PATH` 不存在 → `git clone --filter=blob:none` Vulkan-Docs。
2. 若所需 tag 在本地缺失 → `git fetch --depth=1 origin tag <tag>`。
3. 构建 shard：
   - HEAD（仅 BM25）：约 4–5 秒。
   - HEAD（含 embeddings）：CPU 上约 7 小时（27K 条文本）。开发时用
     `VKQUERY_SKIP_EMBED=1` 或 `VKQUERY_EMBED_LIMIT=N` 加速。

stderr 是 TTY 时（在终端直接跑 `vkquery`），shard 构建期间会看到一个
带阶段名的 spinner（如 `Parsing vk.xml registry`、
`Extracting explicit VUIDs from chapters/*.adoc` 等）。embedding 阶段
会切到带 N/total + ETA 的进度条。把输出 pipe 走、从 MCP 客户端启动、
或设 `VKQUERY_NO_PROGRESS=1` 时自动静默——不会污染 JSON 输出或 MCP
stdio 协议。

同一 (tag, vk.xml content-hash) 后续查询 <50ms。

### 2.3 Shard 缓存布局

```
<cache>/
  Vulkan-Docs/                  ← git clone
  tags-index.json               ← {tag → 最新 shard 信息}
  tags/
    HEAD/<vkxml_sha[:12]>/
      manifest.json
      functions.json structs.json handles.json enums.json
      extensions.json features.json aliases.json reverse.json
      vuids.json                ← explicit + implicit 合并
      bm25/{docs.json, meta.json}
      embeddings/{vectors.f32, meta.jsonl, model.txt}   ← 可选
      prose/...
```

Python 与 Rust 实现 shard 完全互通——可以用 Python 构建、Rust 查询，反之亦然。

### 2.4 预构建 shard（可选）

每个 tag 的首次查询会触发完整 shard 构建（XML 解析 + BM25 语料 + 可选 embedding），
冷启动数分钟到数小时。为跳过这一次性开销，可直接下载预构建 slim shard 解压到
缓存目录：

```bash
# 1. 查看 cache 位置（不需要预先建过 shard 也能查）
vkquery cache info

# 2. 从 shards-latest 滚动 release 下载对应 tag 的 tarball
curl -L -o vkquery-shard-v1.4.352-slim.tar.gz \
  https://github.com/w6rsty/vkquery-rs/releases/download/shards-latest/vkquery-shard-v1.4.352-slim.tar.gz

# 3. 解压进 $VKQUERY_CACHE_DIR（归档根目录是 tags/<tag>/<sha[:12]>/...）
tar -xzf vkquery-shard-v1.4.352-slim.tar.gz -C "$VKQUERY_CACHE_DIR"
```

预构建 shard 由 `.github/workflows/shards.yml` 每周一 06:00 UTC 自动刷新，覆盖
**HEAD + 最近 5 个 v1.x.y tag**；每次 `v*` 二进制 release 也会附带同一批 shard
作为资产。归档内容**只含 BM25 + XML 索引**，不含 embeddings 层——使用
`--mode embed` 或 `--mode hybrid` 时仍需本地补一次 embedding：

```bash
vkquery index build --tag v1.4.352 --force   # 复用已有 BM25 + XML，仅跑 embedding
```

**已知限制**：即便 shard 已落地，第一次查询仍会调用 `git rev-parse` 解析
tag → 提交哈希，所以本地仍需要一个 Vulkan-Docs 克隆（轻量；`vkquery cache info`
只需查 shard 元信息不触发克隆）。Shard 节省的是 XML 解析 + 语料构建的几分钟，
不是 git 克隆的几十秒。

Windows 用户：`tar.exe` 在 Win10+ 内置，PowerShell 中上述命令同样可用。

## 三、CLI 命令

所有命令默认输出格式化 JSON。`--tag <v1.x.y>` 可省略（默认 `HEAD`）。

### 3.1 八大查询

#### `function <name>` —— 查询命令

返回签名、参数、queue/renderpass 限制、所有 VUIDs、可用版本。

```bash
vkquery function vkCmdDraw
vkquery function vkCmdDraw --tag v1.4.352
```

输出关键字段：
- `name`, `return_type`, `params[]`
- `queues[]`（如 `graphics`/`compute`/`transfer`）
- `renderpass`（`inside` / `outside` / `both`）
- `cmdbufferlevel[]`, `tasks[]`
- `vuids[]`（implicit + explicit）
- `available_in[]`（如 `["VK_VERSION_1_0"]`）
- `aliases[]`, `aliased_from`

#### `struct <name>` —— 查询结构体

返回成员、structextends（这个结构体扩展谁）、extended_by（谁扩展这个结构体）、VUIDs。

```bash
vkquery struct VkImageCreateInfo
vkquery struct VkPhysicalDeviceFeatures2 --tag v1.3.250
```

`extended_by[]` 列出所有可放入这个结构体 `pNext` 链的类型，对理解 pNext 链至关重要。

#### `extensions [filters]` —— 列出扩展

```bash
vkquery extensions                                       # 所有扩展
vkquery extensions --type device                         # 仅 device 扩展
vkquery extensions --author KHR                          # 仅 KHR 作者
vkquery extensions --status active                       # 仅活跃扩展
vkquery extensions --type device --author KHR --status active   # 组合过滤
```

`--status` 可选值：`active` / `promoted` / `deprecated` / `obsoleted`。

#### `diff <v1> <v2> [--entity <kind>]` —— 版本对比

```bash
vkquery diff v1.3.250 v1.4.350                                # 全量 diff
vkquery diff v1.3.250 v1.4.350 --entity functions             # 仅命令
vkquery diff v1.3.250 v1.4.350 --entity features              # 仅 VK_VERSION_*
```

`--entity` 可选：`functions` / `structs` / `enums` / `handles` / `extensions` /
`features` / `vuids`。输出含 `added` / `removed` / `changed` / `promoted` 四类。

#### `callers <type>` —— 查找消费者

```bash
vkquery callers VkImage
vkquery callers VkDescriptorSetLayout --tag v1.4.352
```

列出所有以该类型作为参数或成员的命令和结构体（包含 alias 链）。

#### `deps <function>` —— 查询命令依赖图

```bash
vkquery deps vkCmdDraw
vkquery deps vkCreateGraphicsPipelines --tag v1.4.352
```

返回完整依赖图：父 handle 链、必需的 features/extensions、pNext 接受的类型、
externsync 参数、queue/renderpass 约束。代码生成与正确性校验用得到。

#### `vuid <id>` —— 查询单个 VUID

```bash
vkquery vuid VUID-vkCmdDraw-None-02691
vkquery vuid VUID-VkImageCreateInfo-sType-sType
```

返回：规则文本、来源文件、守卫扩展（哪些 ifdef 守护这条规则）、所属实体、
implicit/explicit 分类、可用版本。

#### `search <query> [--mode <…>] [-k <N>]` —— 全文检索

```bash
vkquery search "image layout transition"                          # 默认 hybrid，k=10
vkquery search "image layout transition" --mode bm25 -k 5         # 纯词法 BM25
vkquery search "command buffer recording" --mode embed -k 3       # 纯语义嵌入
vkquery search "render pass compatibility" --mode hybrid -k 5     # RRF 融合
```

检索范围：章节文本（prose）+ VUID 文本。

- `bm25`：rank_bm25 算法的 Rust 重实现（k1=1.5, b=0.75, eps=0.25），3-变体
  tokenizer（原始 + 小写 + CamelCase 拆分）。
- `embed`：候 candle 0.8 + bge-small-en-v1.5，CLS-pooled L2-normalized。
- `hybrid`：BM25 与 embedding 各取 top-2K 后做 RRF（c=60）融合。

### 3.2 索引管理

```bash
# 构建 / 重建 shard
vkquery index build --tag HEAD                          # 默认 HEAD
vkquery index build --tag v1.4.352 --force              # 强制重建
vkquery index build --all                               # HEAD + 所有 v1.* tag
vkquery index build --latest 5                          # 最近 5 个 tag

# 列出现有 shard
vkquery index list

# 列出 cache 根目录 + 每个 shard 的 tag / 内容哈希 / 体积 / 构建时间
vkquery cache info

# 清理旧 shard（保留最近 N 个 + HEAD）
vkquery index gc --keep-last 10
```

### 3.3 Vulkan-Docs 仓库管理

```bash
vkquery docs path        # 打印克隆路径
vkquery docs update      # git fetch + rebase main
vkquery docs tags        # 列出本地可发现的 v1.* tags
```

### 3.4 MCP stdio 服务

```bash
vkquery mcp              # 启动 stdio 传输
```

详见第五节。

## 四、Rust 库接口

`lib.rs` 公开 `vkquery::api::*`、`vkquery::types::*`、`vkquery::cache::*`、
`vkquery::index::diff::*` 等模块。

### 4.1 基本调用

```rust
use vkquery::api;

let f = api::get_function("vkCmdDraw", "HEAD")?;
println!("{:?} {} VUIDs", f.queues, f.vuids.len());

let s = api::get_struct("VkImageCreateInfo", "v1.4.352")?;
println!("extends: {:?}", s.extended_by);

let exts = api::list_extensions(
    "HEAD",
    Some("device"),
    Some("KHR"),
    Some("active"),
)?;

let cs = api::find_callers("VkImage", "HEAD")?;
let dg = api::find_dependencies("vkCmdDraw", "HEAD")?;
let v  = api::get_vuid("VUID-vkCmdDraw-None-02691", "HEAD")?;
let hits = api::search_concept("image layout transition", "HEAD", 5, "bm25")?;

let diff = vkquery::index::diff::diff_versions(
    "v1.3.250",
    "v1.4.350",
    Some("features"),
)?;
```

返回值是 `Result<T, anyhow::Error>`；`T` 都实现了 `serde::Serialize` /
`Deserialize`，可直接序列化为 JSON。

### 4.2 自定义缓存 / 数据源

需要在测试或工具里隔离缓存时：

```rust
use vkquery::cache::Cache;
use vkquery::docs_source::DocsSource;

let cache  = Cache::new(Some(std::path::PathBuf::from("/tmp/vkq")));
let source = DocsSource::new(Some(std::path::PathBuf::from("C:/dev/Vulkan-Docs")));
let shard  = vkquery::index::build::build_shard(&source, &cache, "HEAD", true)?;

// 直接读取 shard
let bm25 = vkquery::search::bm25::Bm25::load(&shard.bm25_dir())?;
let hits = bm25.search("image layout transition", 5);
```

`api::*` 默认走全局缓存；需要进程隔离时通过 `VKQUERY_CACHE_DIR` 环境变量更
方便，无须改代码。

### 4.3 公开类型

| 类型 | 用途 | 关键字段 |
|---|---|---|
| `FunctionInfo` | `get_function` 返回值 | `params`, `queues`, `renderpass`, `vuids`, `available_in` |
| `StructInfo` | `get_struct` 返回值 | `members`, `structextends`, `extended_by`, `vuids` |
| `ExtensionInfo` | `list_extensions` 列表元素 | `type`, `author`, `status`, `promotedto`, `provides_*` |
| `DepGraph` | `find_dependencies` 返回值 | `parents`, `required_features`, `required_extensions`, `pnext_accepts` |
| `CallersResult` | `find_callers` 返回值 | `commands`, `structs` |
| `Vuid` / `VuidInfo` | VUID 描述 | `id`, `entity`, `text`, `kind`, `guard_extensions` |
| `SearchHit` | `search_concept` 列表元素 | `score`, `kind` (Section/Vuid), `section_anchor`, `entity_hint`, `snippet` |
| `DiffReport` | `diff_versions` 返回值 | `added`, `removed`, `changed`, `promoted` |

## 五、MCP stdio 服务

`vkquery mcp` 用 stdio 传输运行 8 个 MCP 工具，可被任意 MCP 客户端
（Claude Desktop、Claude Code 等）调用。

### 5.1 客户端配置示例

Claude Desktop / Claude Code 在 `mcp` 配置块里加：

```json
{
  "mcpServers": {
    "vkquery": {
      "command": "C:\\dev\\vkquery-rs\\target\\release\\vkquery.exe",
      "args": ["mcp"]
    }
  }
}
```

### 5.2 已注册的工具

| Tool | 等价 CLI |
|---|---|
| `vk_get_function(name, tag?)` | `vkquery function …` |
| `vk_get_struct(name, tag?)` | `vkquery struct …` |
| `vk_list_extensions(tag?, type?, author?, status?)` | `vkquery extensions …` |
| `vk_diff_versions(v1, v2, entity?)` | `vkquery diff …` |
| `vk_find_callers(type, tag?)` | `vkquery callers …` |
| `vk_find_dependencies(function, tag?)` | `vkquery deps …` |
| `vk_get_vuid(vuid_id, tag?)` | `vkquery vuid …` |
| `vk_search_concept(query, tag?, k?, mode?)` | `vkquery search …` |

所有工具都返回 text content，内容是对应 dataclass 的 JSON 序列化。

### 5.3 命令行 smoke 测试

`rmcp 0.2.1` 的 stdin 行为：管道喂入有限 jsonl 后 EOF 会取消未完成的工具
调用。Smoke 测试需要给服务足够时间响应：

```bash
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
  | (cat; sleep 3) | target/release/vkquery.exe mcp 2>/dev/null
```

期望输出：8 个工具列表，全部以 `vk_` 前缀。

## 六、常见使用场景

### 6.1 验证一个 commit 没破坏 query

```powershell
$env:VKQUERY_CACHE_DIR = "C:\dev\vkquery-rs\target\cache"
cargo test --features mcp
cargo run --features mcp -- function vkCmdDraw --tag HEAD
cargo run --features mcp -- diff v1.3.250 v1.4.350 --entity features
cargo run --features mcp -- vuid VUID-vkCmdDraw-None-02691
cargo run --features mcp -- search "image layout transition" --mode bm25 -k 3
```

### 6.2 在 LLM 工具调用里检索 Vulkan 规范

启动 MCP 服务后，让 LLM：

1. 先 `vk_search_concept("query", mode="hybrid", k=5)` 拿候选段。
2. 命中 entity 后 `vk_get_function(name)` / `vk_get_struct(name)` 拉细节。
3. 校对 VUID 时 `vk_get_vuid(id)` 获取完整文本 + 守卫扩展。
4. 跨版本对比 `vk_diff_versions(v1, v2, entity)` 看 API 演进。

### 6.3 离线生成版本对比报告

```bash
# 预热所有需要的 tag
vkquery index build --latest 10

# 生成 diff JSON
for v in v1.0.0 v1.1.0 v1.2.0 v1.3.0 v1.4.0; do
  vkquery diff $v HEAD --entity features > diffs/$v.json
done
```

### 6.4 在 CI 里 catch implicit VUID 索引漂移

`tests/fixtures/implicit_vuids_head.json` 是 implicit VUID 索引的 golden
快照。改动 `src/index/vuid_implicit.rs` / `src/registry/parse.rs` /
`src/registry/schema.rs` 后，重建 HEAD shard 并 diff fixture，以确认
ID 集没有漂移：

```powershell
$env:VKQUERY_CACHE_DIR = "C:\dev\vkquery-rs\target\cache-ci"
$env:VKQUERY_SKIP_EMBED = "1"
cargo run --release --no-default-features --features mcp -- index build --tag HEAD --force

# 找到 HEAD shard 目录（vkxml_sha 取决于当前 Vulkan-Docs commit）
$shard = (Get-ChildItem "$env:VKQUERY_CACHE_DIR\tags\HEAD" | Select-Object -First 1).FullName

# 对比 ID 集
python -c @"
import json
r = {k:v for k,v in json.load(open(r'$shard\vuids.json')).items() if v.get('kind')=='implicit'}
f = {k:v for k,v in json.load(open('tests/fixtures/implicit_vuids_head.json')).items() if v.get('kind')=='implicit'}
print('only_in_rust:', len(set(r)-set(f)))
print('only_in_fixture:', len(set(f)-set(r)))
"@
```

非零 diff 通常意味着新的 Vulkan-Docs commit 引入了新规则，或本地改动
破坏了 implicit VUID 派生逻辑。

## 七、性能与已知限制

### 7.1 性能指标

| 操作 | 冷 | 暖 |
|---|---|---|
| 构建 HEAD shard（仅 BM25） | ~4s | — |
| 构建 HEAD shard（含 embeddings） | ~7h（CPU） | — |
| 单次 `function` / `struct` 查询 | <100ms | <30ms |
| `search --mode bm25` k=5 | <50ms | <20ms |
| `search --mode embed` k=5 | <500ms | <200ms |

### 7.2 已知限制

- **CPU 嵌入吞吐量低**：Windows 上无 MKL/CUDA 时 BERT 推理 ~32 vec/30s。
  完整 HEAD embedding 约 7 小时。生产场景需要 `--features cuda` /
  `--features mkl`（默认 features 已含 `embed`，但默认走 CPU 后端）。
- **Implicit VUIDs 长尾**：2 条 char-array 静态长度规则未实现
  （`VkShaderInstrumentationMetricDescriptionARM.name/description`）。
  此外的 ID 全部派生，等价于 Vulkan-Docs 自身 `validitygenerator.py` 的输出。
- **预构建 shard 仍需 Vulkan-Docs 克隆**：`vkquery::api::ensure_shard`
  仍调用 `git rev-parse <tag>` 解析路径，所以即便 shard 已落地缓存，第一次
  查询仍需要一个本地 Vulkan-Docs 克隆。Shard 节省的是 XML 解析 + 语料
  构建的开销，不是 git clone 的开销。

## 八、测试

```powershell
$env:VKQUERY_CACHE_DIR = "C:\dev\vkquery-rs\target\cache"
$env:VKQUERY_SKIP_EMBED = "1"

# 全量测试（32 个：22 lib + 9 integration + 1 BM25 fixture）
cargo test --features mcp

# 只跑单元测试（22 个，<1s）
cargo test --features mcp --lib

# 只跑集成测试
cargo test --features mcp --test integration

# 只跑 BM25 fixture（与 tests/fixtures/bm25_top5_head.json 对比 top-5 命中）
cargo test --features mcp --test parity_bm25

# 只跑 embedding sanity（5 句话余弦相似度自洽）
cargo test --features mcp --test embeddings_sanity
```

集成测试假设同级目录有 `..\Vulkan-Docs` 克隆且至少包含 HEAD 与 `v1.3.250`。
没有 v1.0.40-core 时相关测试会自动跳过。
