# 架构

> 适合在你第一次接触这套代码、想先把数据流和设计取舍弄清楚时阅读。
> 已经会用的可以直接看 [usage.md](usage.md)；要改代码请先读
> [extending.md](extending.md)；定位 cache schema 看
> [data-model.md](data-model.md)。

## 问题

Vulkan-Docs 是权威 spec（asciidoc + XML registry），但它是给人读 + 给
Asciidoctor 编译的，不是为程序化检索设计的。要回答「`vkCmdDraw` 在
v1.4.352 里的参数」「v1.3.250 → v1.4.350 之间改了什么」「哪些命令
消费 `VkImage`」这类问题，目前要么手撕 vk.xml，要么等一次完整的
asciidoctor 构建产出 `validusage.json`。两条路都不适合交互式查询。

`vkquery` 在 Vulkan-Docs 之上加一层索引检索，三种使用方式共享同一份
缓存：Rust 库、CLI、MCP stdio 服务。

## 8 条核心设计决策

### 1. 独立仓库，不内嵌进 Vulkan-Docs

Vulkan-Docs ≥ 每周一发，vendored fork 跟不上。我们独立维护、自动管理
Vulkan-Docs 克隆、按需读 git tag 上的快照。这样：

- 上游 schema 变化自动流入。
- 不会落后于 tag。
- 用户可以用 `VKQUERY_DOCS_PATH` 把克隆指到一个固定 commit。

### 2. 纯 Rust vk.xml 解析，遗留 schema 单点修复

`src/registry/parse.rs` 用 `roxmltree` 做纯 Rust XML 解析，不依赖
Vulkan-Docs 自带的 `scripts/reg.py`。理由：

- 「单二进制零依赖」的目标要求查询路径上没有 Python / Ruby。
- vk.xml 的 schema 结构十年来变化不算激烈，集中在
  `src/registry/legacy.rs::repair()` 处理已知遗留 schema
  （例如 v1.3.x 及更早 funcpointer 的 `<name>` 元素位置不同），
  其他部分 schema-agnostic。

代价：要持续跟踪上游对 vk.xml 的非破坏性扩展。每一处 `legacy::repair()`
分支顶上写明它修哪个 tag 范围。

### 3. Per-tag content-addressed shards

每个 git tag 构建一个独立的 shard 目录：

```
%LOCALAPPDATA%\vkquery\tags\<tag>\<vkxml_sha[:12]>\
```

用 `xml/vk.xml` 的内容哈希作为后缀的好处：

- 即使 tag 被强推（Vulkan-Docs 罕见但可能），缓存自动失效。
- 同一 tag 在不同时间点可以并存多个 shard；最新的记在
  `tags-index.json` 里。
- `cache.is_fresh(shard, required)` 是唯一的重建门控。新增索引时
  必须更新 `src/index/build.rs` 的 `XML_INDEX_NAMES` 常量——这是
  freshness 检查的入口。

### 4. 用 `git cat-file --batch` 做懒构建

一次 shard 构建要读 `xml/vk.xml` + 所有 `chapters/*.adoc`。最直觉的
做法是逐文件 `git show <tag>:<path>`，但这要 ~85 次 subprocess 启动，
进程开销占主导。

我们改用一个常驻的 `git cat-file --batch` 子进程
（`src/git.rs` 的 `TagReader`），所有 blob 通过同一条 stdin/stdout
管道流式读取。净效果：单 tag 构建从 ~30s 降到 ~5s。

`TagReader` 是 RAII 类型——一次构建打开一次，结束 drop；不要在多个
tag 之间共用同一个 reader。

### 5. VUIDs：纯 Rust 重导出，无 Ruby

Vulkan-Docs spec build 跑 asciidoctor + Ruby 扩展
（`config/vu-to-json/extension.rb`）才能产出 `validusage.json`。这需要
本地装 Ruby + asciidoctor，外加完成一次 spec build。我们用两条
纯 Rust 路径绕过：

- **Explicit VUIDs**——`src/index/vuid_explicit.rs` 用 regex 扫
  `chapters/*.adoc`，维护一个 `ifdef::EXT[]` 栈，所以每个 VUID 都带
  上守卫扩展集合。`include::{chapters}/commonvalidity/<file>.adoc[]`
  递归展开，把外层 refpage 的实体名替换进 `{refpage}` 占位符。覆盖
  HEAD 上全部 19,833 条 VUID。
- **Implicit VUIDs**——`src/index/vuid_implicit.rs` 从 vk.xml 的属性
  纯 Rust 重导出 `ValidityOutputGenerator` 的规则。覆盖 6,575 条 ID
  中的 6,573；缺的 2 条是 `VkShaderInstrumentationMetricDescriptionARM`
  的 char[N] 静态数组文本（待补）。

代价：要持续跟踪上游 generator 的内部契约——上游若改规则的写法，
我们的 derivation 会偏离。校对方式是把派生结果 diff 到 fixture
`tests/fixtures/implicit_vuids_head.json`（流程见 usage.md 6.4）。

### 6. 反向索引一次 XML 扫描产出

`find_callers(VkImage)` 要的是反向：哪些命令把它当参数，哪些 struct
把它当成员？XML 不存这个数据，我们在 `src/index/reverse.rs` 里一次
遍历 `registry.commands` + `registry.types` 把每个 `<param>` /
`<member>` 按类型名（alias-normalize 后）分桶建出来。同一次扫描也
建出 `extended_by`（反向的 `structextends`，给 pNext 链查询用）。

写到 `reverse.json`，结构：

```json
{
  "consumers": {
    "VkImage": {
      "commands": ["vkDestroyImage", "..."],
      "structs": ["..."]
    }
  },
  "extended_by": {
    "VkImageCreateInfo": ["VkExternalMemoryImageCreateInfo", "..."]
  }
}
```

### 7. 搜索：BM25 默认，embedding 可选

`search_concept` 三种模式：

- **`bm25`**——`src/search/bm25.rs` 是 Okapi BM25 的纯 Rust 实现
  （k1=1.5, b=0.75, eps=0.25，参数与 `rank_bm25.BM25Okapi` 默认一致，
  便于复现常规论文/教程里的 baseline）。tokenizer 是 3 变体：原始 +
  小写 + CamelCase 拆分，所以
  `"image create info"` 能匹配 `"VkImageCreateInfo"`。语料 = 章节
  prose + VUID 文本。无任何外部依赖。
- **`embed`**——`src/search/embedding.rs` 用 candle 0.8 + bge-small-en-v1.5
  做 CLS-pooled L2-normalized 向量。所有 GPU backend（cuda/cudnn/mkl/
  accelerate/metal）通过 cargo feature 透传给 candle，不在编译时硬绑。
  特性须在 build 时启用（`--features embed`），否则代码不编译——避免
  让 BM25-only 用户也吞 candle 的 5MB 体积。
- **`hybrid`**——`src/search/hybrid.rs` 对 BM25 与 embedding 的两路
  top-k 做 Reciprocal Rank Fusion（RRF，c=60）。默认模式。如果 embed
  没启用，hybrid 在 API 层会显式报错（编译期就过滤掉了）。

「lexical 默认、semantic 选启用」是有意为之的：大多数 Vulkan 问题
关键词性强，embedding 主要是给同义词 / 拼写差异兜底，付出 130MB 模型
下载 + 5MB 二进制不一定划算。

### 8. 三个前端，一个核心

```
                 ┌─── CLI (vkquery <subcmd>)
                 │
api::* 函数 ─────┼─── Rust 库 (`use vkquery::api::*;`)
                 │
                 └─── MCP server (vkquery mcp)
```

三套前端都只调 `src/api.rs` 里的公开函数。CLI 与 MCP 是薄适配器：
`src/cli.rs` 用 clap-derive、`src/mcp_server.rs` 用 `rmcp 0.2` 的
`#[tool_router]` + `#[tool_handler]` 宏。

## 一次典型查询的数据流

调 `api::get_function("vkCmdDraw", "v1.4.352")` 时：

1. `api::get_function` → `ensure_shard(tag)` 拿到 `Shard` 句柄。
2. `Cache::shard_for(source, tag)` 解析出 tag 的 commit SHA 与
   `xml/vk.xml` 的 blob SHA，拼出 shard 目录路径。
3. `Cache::is_fresh(shard, XML_INDEX_NAMES)` 判断 manifest 是否齐全
   且 builder_version 匹配。
4. 不 fresh → `index::build::build_shard(source, cache, tag, force)`
   触发构建。
5. `TagReader::open()` 拉起 `git cat-file --batch`；读 `xml/vk.xml`
   + `chapters/*.adoc` 全部 blob。
6. `registry::parse_registry(xml)` 解析为 owned `Registry`；遗留
   schema 在解析前由 `legacy::repair` 修复。
7. `index::xml_index::build_all(&reg)` → 7 个 JSON（functions /
   structs / handles / enums / extensions / features / aliases）。
8. `index::reverse::build_reverse(&reg, &aliases)` → `reverse.json`。
9. `index::vuid_explicit::extract_from_chapters(...)` +
   `index::vuid_implicit::derive_from_registry(...)` → 合并后写
   `vuids.json`，同时回写到 functions/structs 的 `vuid_refs`。
10. `index::prose::build_sections(blobs)` → `prose/chapters.jsonl`。
11. `search::bm25::Bm25::build(sections, vuids).save(...)` →
    `bm25/{docs.json, meta.json}`。
12. （仅 `--features embed`）`search::embedding::build_index(...)` →
    `embeddings/{vectors.f32, meta.jsonl, model.txt}`。
13. `Shard::write_manifest()` 写 `manifest.json`，`tags-index.json`
    更新。
14. 回到查询：读 `functions.json` 找 `"vkCmdDraw"`，按 `vuid_refs`
    从 `vuids.json` 取规则文本，组装成 `FunctionInfo` 返回。

第 5–13 步只在 (tag, vk.xml SHA) 第一次出现时跑一次；之后所有命中
缓存的查询返回都 < 50ms。

## 故意不做的事

- **写回 Vulkan-Docs**。vkquery 只读。
- **多 API**。只构 `api="vulkan"`；vulkansc/vulkanbase 过滤的钩子在
  `registry::parse` 里已经有，但没透到 CLI/API。
- **`video.xml`**。Vulkan Video registry 的 `StdVideo*` 类型还没合并
  到 shard。规划：单独 parse 一份 Registry，按 `category:"stdvideo"`
  标记后合并。
- **跨 tag VUID 谱系**（一个 VUID 是哪个版本引入的？）。理论上对所有
  tag pair 做 diff 可以算出来，但代价线性放大；按需做。
- **Web UI**。CLI + MCP 已经覆盖会话式使用场景。
