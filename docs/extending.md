# 扩展开发指南

> 这份文档约束未来对 `vkquery-rs` 做修改的所有贡献者。先读完
> [architecture.md](architecture.md) 再来这里。

## 黄金规则

1. **别自己另写一份 vk.xml 解析器。** 用 `src/registry::parse_registry`
   返回的 `Registry`。如果 `src/index/xml_index.rs` 还没暴露你要的字段，
   去那个模块加一个，而不是再起一个 `roxmltree::Document`。
   *理由*：并行解析器会漂——vk.xml 的边角（alias 链、api/profile 过滤、
   structextends 反向、`apitype="internal"` 子 feature 合并）只有
   一份处理才能保证语义一致。
2. **写盘只走 `Shard`。** `src/cache.rs` 的 `Shard::write_json`、
   `Shard::write_manifest`、`Shard::bm25_dir() / embeddings_dir()` 子目
   录是仅有的合法写入路径。绕过它们会跳过 freshness 检查，下次
   `is_fresh` 不知道你写了什么，结果是 cache miss 永远命不中。
3. **不要随便 bump `BUILDER_VERSION`。** 每次 bump 会使所有用户机器上
   的 shard 失效。只在以下情况 bump：
   - 你改了某个 JSON 文件的 schema（字段增删、语义变化）。
   - 你修了一个 bug，而错误的输出还残留在缓存里。

   仅新增字段的可加性修改通常不需要 bump，但拿不准的时候按「bump」处理。
4. **新增可选依赖必须是 cargo feature。** 在 `Cargo.toml` `[features]`
   下挂一个 feature，引用代码用 `#[cfg(feature = "foo")]` 隔离。模式：

   ```rust
   #[cfg(feature = "foo")]
   pub fn enable_foo() { /* ... */ }

   #[cfg(not(feature = "foo"))]
   pub fn enable_foo() -> ! {
       panic!("foo feature not compiled in; rebuild with --features foo")
   }
   ```

   宁可在编译期排除，也别在运行时做「软依赖」。Rust 的 cfg 机制就是
   为这个场景设计的。
5. **公开类型即 API。** `src/types.rs` 里的所有 `pub struct`（以及
   `Vuid` / `SearchHit` 等）都被库消费者、CLI、MCP tool schema、测试
   断言读取。新增字段必须给默认值（`#[serde(default)]`），永远不
   重命名或删除字段，除非主版本号也跟着升。

## 怎么加一个新的 query primitive

worked example：加一个 `find_synonyms(name)` 返回 alias + promoted-from
链。

1. **在 `src/types.rs` 定义返回类型。** 每个字段给默认值，构造保持
   可加性。
   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize, Default)]
   pub struct SynonymInfo {
       pub name: String,
       #[serde(default)]
       pub aliases: Vec<String>,
       #[serde(default)]
       pub promoted_from: Option<String>,
   }
   ```
2. **在 `src/api.rs` 实现。** 模式：
   ```rust
   pub fn find_synonyms(name: &str, tag: &str) -> Result<SynonymInfo> {
       let shard = ensure_shard(tag)?;
       let aliases: BTreeMap<String, String> = shard.read_json("aliases")?;
       // ... 读你要的 shard 文件
       Ok(SynonymInfo { ... })
   }
   ```
   只读已经在 shard 里的数据。要新数据 → 第 4 步。
3. **接 CLI。** `src/cli.rs` 加 `Cmd` enum 分支 + match arm 调
   `api::*`，输出走 `emit()`。
4. **如果需要 shard 新数据：**
   - 在 `src/index/xml_index.rs`（或单独建一个新模块）写一个 build
     函数，产出一个 `BTreeMap<String, _>` 按实体名 key。
   - 把 JSON 文件名加进 `src/index/build.rs` 的 `XML_INDEX_NAMES`
     常量——这是 freshness 检查的列表。
   - 在 `build_shard` 里调你的 builder + `shard.write_json("name",
     &payload)?`。
   - 在 [data-model.md](data-model.md) 加一节描述 schema。
   - bump `src/cache.rs::BUILDER_VERSION`。
5. **接 MCP。** `src/mcp_server.rs` 加 `#[tool(description = "...")]`
   async fn + Parameters struct。
6. **加测试。** `tests/integration.rs` 跑一次端到端 happy path；
   src 内 `#[cfg(test)]` 模块加 unit-level 校验。
7. **更新 `CLAUDE.md` 的 query 表 + 相关 `docs/`。**

## 怎么支持一个新的 git-tag schema

如果某个 tag 在 `parse_registry` 里失败：

1. 复现：
   ```powershell
   $env:VKQUERY_CACHE_DIR = "C:\dev\vkquery-rs\target\cache-debug"
   cargo run -- index build --tag vX.Y.Z --force
   ```
2. 定位差异：手动 dump `xml/vk.xml` for that tag，跟一个能 parse 的 tag
   diff，找到 schema 区别。
3. 在 `src/registry/legacy.rs::repair()` 里加补丁分支。`repair()` 接受
   原始 XML 字符串，**在 parse 之前**做字符串级修正，例如把
   `<type category="funcpointer"><name>X</name>` 包装成
   `<type category="funcpointer"><proto><name>X</name></proto>`。每个补
   丁顶部注明覆盖的 tag 范围。
4. 加测试：把那个 tag 的 vk.xml 片段做成 fixture，跑 `parse_registry`
   断言成功。如果是历史 tag（如 v1.0.40-core），可以 sibling-clone 检查
   后 skip，模仿 `tests/integration.rs::parses_v1_0_40_core_vk_xml`。

## 怎么加一个新的搜索后端

worked example：加 Tantivy。

1. 新建 `src/search/tantivy_index.rs`：实现
   `pub fn build_index(sections, vuids, out_dir) -> Result<()>`、
   `pub fn search(dir, query, k) -> Result<Vec<SearchHit>>`，
   模仿 `src/search/bm25.rs` 的 shape。
2. 在 `Cargo.toml` `[features]` 加 `tantivy = ["dep:tantivy"]`；
   `[dependencies]` 加 `tantivy = { version = "...", optional = true }`。
3. 引用代码 `#[cfg(feature = "tantivy")]` 包裹。
4. `src/index/build.rs::build_shard` 末段加一个 cfg 块，调
   `tantivy_index::build_index`。失败时不破整个 shard——独立 try/catch。
5. manifest 加 `tantivy: bool` 字段。
6. `src/api.rs::search_concept` 加 `"tantivy"` 分支。是否进 hybrid
   要单独决策（目前 hybrid 是 BM25 + embed，看是否值得三路 RRF）。
7. 在 `data-model.md` 加新子目录的描述。

## 怎么加一个新前端（HTTP server / gRPC 等）

模式：放在 `src/<name>_server.rs`，**只调** `api::*`，不重复查询逻辑。
参考 `src/mcp_server.rs` 的形状：

- 用 cargo feature 隔离运行时依赖。
- 在 `src/cli.rs` 加一个子命令启动它。
- 测试：dispatch 路径要可独立 unit-test，不依赖真正的 I/O。

## Rust 特有的 pitfalls（前辈踩过）

- **`serde_json::Map` 默认是 BTreeMap-backed**——`json!({...})` 的字段
  顺序按字面量出现顺序写入，但序列化时 key 按字母排序。如果你用
  `#[derive(Serialize)]`，字段顺序变成 struct 定义顺序，可能不是字母
  序——尤其在 shard JSON 写入路径上（cache 是 content-addressed，要保
  证字节稳定），**手动确保字段按字母序声明**，或者转走 `json!` 宏。
- **`half::bf16: SampleUniform` 冲突**——candle 0.7 与 `rand 0.9` 联合
  使用时报这个 trait bound 错误。已升级到 candle 0.8 解决；保留这条
  以防回归。
- **`opt-level = "z"` 让 BERT 推理变成 60s/batch**——`Cargo.toml`
  `[profile.release]` 用 `opt-level = 3`（不是 size），LTO + strip 后
  二进制仍然 ~3.7MB（slim）/ ~13MB（默认含 embed）。不要为了体积切回
  `"z"`。
- **Windows path separator**——代码统一用 `PathBuf::join`；不要在
  字符串里硬拼 `/` 或 `\`。`Cache::shard_dir(tag, vkxml_sha)` 已经处理
  了 tag 名里 `/`（如 `release/v1.4.x`）的转义。
- **CRLF newlines**：`tests/parity_bm25.rs` 读 `tests/fixtures/*.json`
  时是 string + serde_json 解析，CRLF-safe。BM25 tokenization 跑的
  字节来自 `git cat-file --batch`（git 对象存储的原始 blob），不经
  worktree 的 `core.autocrlf` 转换——所以 Linux/Windows 构建出的
  shard 字节完全一致。
- **`rmcp 0.2.1` 的 stdin EOF 行为**：有限 jsonl 输入 + EOF 会取消
  未完成的 tool 调用。本地 smoke 测试需要给服务时间：
  `(cat req.jsonl; sleep 3) | vkquery mcp`。
- **`#[cfg(feature = "embed")]` 的 doc-test 陷阱**：如果在 `embed`
  feature gate 的代码里写 doctest，`cargo test --no-default-features`
  下会跑这个 doctest 的字符串（doctest 不看 cfg），导致编译错误。
  embed 模块里目前没有 doctest；新增时确认 doctest 内部也 cfg-gate。
- **`#[serde(default)]` 是兼容性铠甲**——所有 `Option<T>` 和
  `Vec<T>` 字段必须加，否则旧 shard JSON 缺该字段时反序列化报错。

## 拿不准的时候

- 先去看正在改动区域的现有测试。没测试就先写一个。
- 用户偏好「函数形态、紧凑」的代码风格，不喜欢提前抽象。除非现在某段
  代码确实放错模块，否则不要单独建新 mod。
- 如果你在实现过程中突然觉得「这里如果加个 feature flag / 配置项会
  更好」——先问再加，不要主动加任何用户没要求的 flag。
