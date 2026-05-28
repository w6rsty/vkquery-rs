# 使用手册

`vkquery-rs` 是 Khronos Vulkan-Docs 的查询层，单二进制即可运行。三套前端
共享同一份缓存：CLI、Rust 库、MCP stdio 服务。本文档覆盖所有可用功能与
具体使用方法。

> 当前状态：R0–R7 全部完成；R5 implicit VUIDs 与 Python 参考实现的 ID
> 一致率 100.00% / 准确率 100.00%（6575/6575）；R6 BM25 通过形式化 parity 测试。
> 共 33 个测试通过（`cargo test --features mcp`）。

## 一、构建与安装

### 1.1 从源码构建

```bash
cd C:\dev\vkquery-rs

# 默认配置 = mcp + embed（约 8MB，含 BM25 / 语义搜索 / MCP 服务）
cargo build --release

# 仅 BM25 + MCP（约 2.5MB，不含 candle BERT 嵌入）
cargo build --release --no-default-features --features mcp

# 最小（仅库 / CLI，不含 MCP 与嵌入）
cargo build --release --no-default-features
```

可选 cargo features：

| Feature | 作用 | 体积影响 |
|---|---|---|
| `mcp` | 启用 `vkquery mcp` 命令；引入 `rmcp 0.2` + `tokio` | +约 1MB |
| `embed` | 启用语义搜索；引入 `candle-core 0.8` + `tokenizers` + bge-small-en-v1.5 | +约 5MB（不含模型权重） |

二进制位于 `target/release/vkquery.exe`（Windows）或 `target/release/vkquery`。

### 1.2 用 cargo install 安装

```bash
cargo install --path C:\dev\vkquery-rs
# 或指定特性
cargo install --path C:\dev\vkquery-rs --features mcp --no-default-features
```

### 1.3 直接下载预编译二进制

每个 `v*` tag 会触发 `.github/workflows/release.yml` 在三个平台
（`x86_64-unknown-linux-gnu`、`aarch64-apple-darwin`、`x86_64-pc-windows-msvc`）
构建仅含 `mcp` 特性的精简二进制（约 2.5MB），并连同 SHA256 校验上传到
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

### 2.2 首次运行行为

第一次查询任何 tag 会触发以下步骤：

1. 若 `VKQUERY_DOCS_PATH` 不存在 → `git clone --filter=blob:none` Vulkan-Docs。
2. 若所需 tag 在本地缺失 → `git fetch --depth=1 origin tag <tag>`。
3. 构建 shard：
   - HEAD（仅 BM25）：约 4–5 秒。
   - HEAD（含 embeddings）：CPU 上约 7 小时（27K 条文本）。开发时用
     `VKQUERY_SKIP_EMBED=1` 或 `VKQUERY_EMBED_LIMIT=N` 加速。

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

### 6.4 在 CI 里 catch implicit VUID 回归

Rust shard 与 Python shard 完全互通，可以 cross-verify：

```bash
# 1) 构建 Rust shard
$env:VKQUERY_CACHE_DIR = "C:\dev\vkquery-rs\target\cache-ci"
cargo run --release -- index build --tag HEAD --force

# 2) 用 Python 加载并 diff
PYTHONPATH=C:\dev\vkquery\src python target/parity_r5.py
```

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
  `--features mkl`（当前未启用）。
- **R5 implicit VUIDs 长尾**：2 条 char-array 静态长度规则未实现
  （`VkShaderInstrumentationMetricDescriptionARM.name/description`）。
  其他 ID 完全对齐 Python 参考实现。
- **文本漂移 7.9%**：implicit VUIDs 文本与 Python 在空格 / "the" /
  复数等微差异上略有不同。ID 完全一致，下游做精确字符串匹配时才有影响。
- **R8 未完成**：没有 CI 工作流、跨平台预编译、GitHub Release。

参见 `CLAUDE.md` 与 `tests/fixtures/implicit_vuids_head.json` 获取 parity
状态详情。

## 八、测试

```powershell
$env:VKQUERY_CACHE_DIR = "C:\dev\vkquery-rs\target\cache"
$env:VKQUERY_SKIP_EMBED = "1"

# 全量测试（33 个）
cargo test --features mcp

# 只跑单元测试（22 个，<1s）
cargo test --features mcp --lib

# 只跑集成测试
cargo test --features mcp --test integration

# 只跑 BM25 parity（与 Python top-5 对比）
cargo test --features mcp --test parity_bm25

# 只跑 embedding sanity
cargo test --features mcp --test embeddings_sanity
```

集成测试假设同级目录有 `..\Vulkan-Docs` 克隆且至少包含 HEAD 与 `v1.3.250`。
没有 v1.0.40-core 时相关测试会自动跳过。
