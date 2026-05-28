# 数据模型

> 一份 shard 就是某个 (Vulkan-Docs tag, `xml/vk.xml` content-hash) 的
> 自包含快照。cache miss 时 `src/index/build.rs::build_shard` 一次性
> 产出下面所有文件。`builder_version` 字符串（`src/cache.rs` 常量）
> 一旦被 bump，所有旧 shard 视为失效。

本文档列出每个文件的 schema；新增字段或重命名都要同步
[extending.md](extending.md) 的 "Don't bump builder_version casually" 规则。

## 目录布局

```
%LOCALAPPDATA%\vkquery\                  (Windows，其他平台是 ~/.cache/vkquery)
  Vulkan-Docs/                            托管的 clone，可选
  tags-index.json                         {tag: {shard_dir, commit_sha, vkxml_sha, built_at}}
  tags/<tag>/<vkxml_sha[:12]>/
    manifest.json
    functions.json
    structs.json
    handles.json
    enums.json
    extensions.json
    features.json
    aliases.json
    reverse.json
    vuids.json
    prose/chapters.jsonl
    bm25/
      docs.json                            Bm25Doc[]
      meta.json                            { k1, b, epsilon, corpus_size, avgdl }
    embeddings/                            仅 --features embed 构建时存在
      vectors.f32                          row-major little-endian f32, shape [N, dim]
      meta.jsonl                           每行一个 EmbedDoc
      model.txt                            模型 id 字符串（如 BAAI/bge-small-en-v1.5）
```

`BUILDER_VERSION` 当前是 `"0.2.0-rust"`。`Cache::is_fresh()` 拒绝
`builder_version` 或 `vkxml_sha` 不匹配的 shard。

## manifest.json

```json
{
  "tag": "v1.4.352",
  "commit_sha": "e2843a23d3c5...",
  "vkxml_sha": "0241edfe50de...",
  "builder_version": "0.2.0-rust",
  "built_at": 1779937792,
  "command_count": 718,
  "struct_count": 1234,
  "vuid_count": 26408,
  "section_count": 1180,
  "bm25": true,
  "embeddings": false
}
```

`*_count` 字段是构建时统计的快照，主要给 `vkquery index list` 和测试
断言用。

## functions.json

按命令名作为 key 的 map：

```json
{
  "vkCmdDraw": {
    "name": "vkCmdDraw",
    "return_type": "void",
    "params": [
      {
        "name": "commandBuffer",
        "type": "VkCommandBuffer",
        "optional": false,
        "len": null,
        "externsync": "true",
        "noautovalidity": false,
        "const": false,
        "pointer_depth": 0
      },
      ...
    ],
    "success_codes": ["VK_SUCCESS"],
    "error_codes": ["VK_ERROR_OUT_OF_HOST_MEMORY", ...],
    "queues": ["graphics"],
    "renderpass": "inside",
    "cmdbufferlevel": ["primary", "secondary"],
    "tasks": ["action", "drawing"],
    "feature_origin": "VK_VERSION_1_0",
    "available_in": ["VK_VERSION_1_0"],
    "aliases": [],
    "aliased_from": null,
    "vuid_refs": ["VUID-vkCmdDraw-None-02691", ...]
  }
}
```

字段语义：

- `queues` / `cmdbufferlevel` / `tasks` 由 XML 逗号分隔属性派生；
  queue token 归一化（`VK_QUEUE_GRAPHICS_BIT` → `graphics`）。
- `feature_origin` 是**第一个** `<feature>` 或 `<extension>` 在其
  `<require>` 块里声明这个命令的名字；feature 优先于 extension
  （我们按 declaration order 遍历 feature 然后 extension）。
- `available_in` 是声明此命令的 feature + extension 全集。promotion
  到 core 后，扩展和被合并入的 feature 都会出现。
- `aliases`：本命令被谁 alias（反向）。
- `aliased_from`：调用者输入名本身是一个 alias 时填入；
  `get_function("vkQueueSubmit2KHR")` 实际返回 `vkQueueSubmit2`
  的数据，`aliased_from = "vkQueueSubmit2KHR"`。
- `vuid_refs` 由 `index::vuid_explicit::attach_vuid_refs` 在合并 VUIDs
  时回填。规则文本去 `vuids.json` 取。

## structs.json

结构与 functions.json 同形，但来自 `<type category="struct|union">`。
追加字段：

- `category`：`"struct"` 或 `"union"`。
- `returnedonly`：bool。
- `structextends`：本结构体作为 pNext 扩展目标的结构体列表。
- `members`：与 `params` 同形，多一个 `values` 字段（sType
  discriminator，例如 `"VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO"`）。

注意：`extended_by` 不在 structs.json 里——它在 `reverse.json` 中
集中维护。

## handles.json

```json
{
  "VkImage": {
    "name": "VkImage",
    "parent": "VkDevice",
    "dispatchable": false,
    "objtypeenum": "VK_OBJECT_TYPE_IMAGE",
    "aliases": [],
    "aliased_from": null
  }
}
```

- `dispatchable` 来自 XML：`VK_DEFINE_HANDLE` → true，
  `VK_DEFINE_NON_DISPATCHABLE_HANDLE` → false。
- `parent` 顺着 handle 父链向上一级（VkImage → VkDevice →
  VkPhysicalDevice → VkInstance）。

## enums.json

按 `<enums name="...">` 块的 name 作 key：

```json
{
  "VkImageUsageFlagBits": {
    "name": "VkImageUsageFlagBits",
    "type": "bitmask",
    "bitwidth": 32,
    "values": [
      {
        "name": "VK_IMAGE_USAGE_TRANSFER_SRC_BIT",
        "value": null,
        "bitpos": "0",
        "alias": null,
        "comment": null
      },
      ...
    ],
    "aliases": [],
    "aliased_from": null
  }
}
```

`bitpos` 是字符串（与 Python `e.get("bitpos")` 返回原始 XML 属性文本
对齐，不做 int 转换）。

extension 注入的 enumerant（来自 `<require><enum extends=...>`）已经
在 build 阶段被 materialize 到对应 group 的 `values` 里。

## extensions.json

```json
{
  "VK_KHR_surface": {
    "name": "VK_KHR_surface",
    "number": 1,
    "type": "instance",
    "author": "KHR",
    "contact": "James Jones @cubanismo, ...",
    "supported": ["vulkan", "vulkansc"],
    "depends": null,
    "requires_extensions": [],
    "requires_core": null,
    "promotedto": null,
    "deprecatedby": null,
    "obsoletedby": null,
    "status": "active",
    "provides_commands": ["vkDestroySurfaceKHR", ...],
    "provides_types": ["VkSurfaceKHR", ...],
    "provides_enums": ["VK_KHR_SURFACE_SPEC_VERSION", ...]
  }
}
```

- `status` 计算优先级：`obsoletedby` > `deprecatedby` > `promotedto`
  > `"active"`。
- `requires_extensions` 是 `depends` 字符串的天真 token 提取（例如
  `"VK_VERSION_1_1+VK_KHR_a,VK_KHR_b"` → `["VK_KHR_a", "VK_KHR_b"]`）。
  完整的布尔表达式还原暂时不做，因为目前所有用例只关心被提及的扩展
  集合。

## features.json

Vulkan 核心版本（`VK_VERSION_1_0` … `VK_VERSION_1_4`）。**纯
`api="vulkan"` 视角**——`apitype="internal"` 的子 feature（如
`VK_BASE_VERSION_1_0`、`VK_GRAPHICS_VERSION_1_0`）的 `provides_*` 会
聚合到对应公共 feature 上，不单独出现。

```json
{
  "VK_VERSION_1_4": {
    "name": "VK_VERSION_1_4",
    "number": "1.4",
    "depends": "VK_GRAPHICS_VERSION_1_0",
    "provides_commands": [...],
    "provides_types": [...],
    "provides_enums": [...]
  }
}
```

## aliases.json

forward map `{alias: canonical}`。一跳就够；调用方需要传递闭包则在 API
层做递归（`src/api.rs::resolve_alias`）。

## reverse.json

```json
{
  "consumers": {
    "VkImage": {
      "commands": ["vkAcquireImageANDROID", "vkBindImageMemory", ...],
      "structs":  ["VkBindImageMemoryInfo", "VkBlitImageInfo2", ...]
    }
  },
  "extended_by": {
    "VkImageCreateInfo": ["VkExternalMemoryImageCreateInfo", ...]
  }
}
```

排序去重。类型名 alias-normalize 后入桶——所以
`find_callers("VkRenderingInfoKHR")` 会查到 `VkRenderingInfo` 的
消费者。

## vuids.json

按 VUID id 作 key：

```json
{
  "VUID-vkCmdDraw-None-02691": {
    "id": "VUID-vkCmdDraw-None-02691",
    "entity": "vkCmdDraw",
    "param": "None",
    "kind": "explicit",
    "text": "If a VkImageView is accessed using atomic operations as a result of this command, then the image view's format features must: contain VK_FORMAT_FEATURE_STORAGE_IMAGE_ATOMIC_BIT",
    "guard_extensions": [],
    "source_file": "chapters/commonvalidity/draw_dispatch_common.adoc"
  }
}
```

- `kind`：`"explicit"`（chapters/*.adoc 里的项目符号）或 `"implicit"`
  （`vuid_implicit::derive_from_registry` 派生）。
- `guard_extensions`：本 VUID 所在的 `ifdef::EXT[]` 块的扩展名集合。
  空数组 = core，无守卫。
- `param`：本 VUID 关联的参数槽。特殊值 `"None"` 表示「全命令 / 全
  结构体规则，无单一参数」。implicit VUIDs 的后缀是 category
  （`parameter` / `parent` / `cmdpool` / `recording` / `requiredbitmask`
  / `zerobitmask` 等）。
- `source_file`：来源文件。implicit VUIDs 用虚拟路径
  `generated/validity/protos/<entity>.adoc` 或
  `generated/validity/structs/<entity>.adoc`，沿用 Vulkan-Docs
  `validitygenerator.py` 的命名习惯。

explicit 与 implicit 有同 id 时（罕见），explicit 优先——人写的文本
通常更易读。

## prose/chapters.jsonl

每行一个 JSON 对象，每章每节产一行：

```json
{
  "file": "chapters/synchronization.adoc",
  "section_id": "synchronization-image-memory-barriers",
  "heading": "Image Memory Barriers",
  "heading_path": ["Synchronization", "Image Memory Barriers"],
  "text": "... markup-stripped 后的纯文本 ...",
  "refpage_entities": ["VkImageMemoryBarrier2", "VkImageMemoryBarrier"]
}
```

`refpage_entities` 是节内开启的所有 `[open,refpage='X',...]` 块的
实体名集合——hit 命中后可以回链到对应的 function/struct 页。

`src/index/prose.rs` 的拆分规则：

- `=+ Heading` 行做章节切分；level 1 = 文档标题。
- `[[anchor]]` 独占一行设置下一节的 `section_id`。
- `[open,refpage='X']` 把 X 加进当前节的 `refpage_entities`。
- `if(n?)def::` / `endif::` 不参与 prose（VUID 提取才需要 ifdef 栈）。
- 所有文本通过 `util::strip_asciidoc_markup` 去掉行内格式
  （`pname:foo` → `foo`，`<<anchor,text>>` → `text`，等等）。

## bm25/

两个文件：

- **`docs.json`**：`Bm25Doc[]`，每个 doc 是
  ```json
  {
    "kind": "section" | "vuid",
    "source_id": "...",
    "source_file": "chapters/...",
    "section_anchor": "...",
    "entity_hint": "vkCmdDraw" | null,
    "text": "...",
    "tokens": ["image", "Image", "create", "info", "vkimagecreateinfo", ...]
  }
  ```
  `tokens` 是预先 tokenize 好的，加载即可查询。三变体 tokenizer：原始
  + 小写 + CamelCase 拆分。
- **`meta.json`**：`{ k1, b, epsilon, corpus_size, avgdl }`。
  IDF 在加载时按 `Bm25::from_docs` 一次性算出（~22K docs 不到 1s），
  不持久化。

corpus 包含 prose 章节段与 VUID 文本两类 doc，由 `kind` 字段区分。
持久化用纯 JSON 而不是序列化 BM25 模型对象，是为了避免任何特定语言
的反序列化机制（pickle / serde-bincode 等）成为加载 shard 的硬依赖。
IDF 在 `Bm25::from_docs` 加载时一次性算出，~22K docs 不到 1s。

## embeddings/

仅在 `cargo build --features embed` 后才会被填充。

- **`vectors.f32`**：row-major little-endian f32，shape `[N, dim]`。
  `N` = doc 数量，`dim` = 384（bge-small）。L2 normalized——查询时只
  需点积即可得到余弦相似度。
- **`meta.jsonl`**：每行一个 `EmbedDoc`（与 `Bm25Doc` 同形，但少
  `tokens` 字段），行号对应 `vectors.f32` 的 row index。
- **`model.txt`**：UTF-8 单行的模型 id（如
  `BAAI/bge-small-en-v1.5`）。查询时从这个文件读模型，与 build 时一致。

查询用扁平 `Vec<f32>` 暴力点积——22K vec × 384 dim 在 ~3ms 内即可
（即使纯标量实现），单进程 single-shard 工作负载上 ANN 索引（FAISS /
hnswlib 等）的开销并不划算。
