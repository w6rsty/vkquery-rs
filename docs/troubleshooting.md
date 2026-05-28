# 故障排查

> 常见报错、踩坑、和它们的修复方式。遇到新的尖角请添加。

## `Unknown ref: vX.Y.Z`

这个 tag 不在本地 Vulkan-Docs 克隆里。两种修法：

```bash
vkquery docs update                                      # 拉最新 tags + main
# 或者手动
git -C $env:VKQUERY_DOCS_PATH fetch --depth=1 origin tag vX.Y.Z
```

如果 tag 上游确实不存在，先确认拼写——老的 core-only tag 命名是
`v1.0.NN-core` 而不是 `v1.0.NN`。

## shard 构建时报「type without a name」

你撞上了一个 `src/registry/legacy.rs::repair` 没覆盖的遗留 schema。看
[extending.md § 「怎么支持一个新的 git-tag schema」](extending.md)。

## `get_function` / `get_struct` 返回里 VUIDs 缺失

两种可能：

1. **shard 用旧 `BUILDER_VERSION` 构建**（VUID 支持加入前的版本）。
   `vkquery index build --tag <tag> --force` 重建即可。
2. **VUID 在你没预期的 `ifdef::EXT[]` 守卫下**。直接看
   `vuids.json`，找 `guard_extensions` 字段。即使有守卫，`get_function`
   也会返回这条 VUID——是否过滤由消费者决定。

如果你确认 `chapters/X.adoc` 里有这条 VUID 但 `vuids.json` 没它：

- 确认 VUID anchor 匹配 regex `^\s*\*\s+\[\[VUID-...-\d{5}\]\]`。
- 如果在 `commonvalidity/` 下，确认 refpage 属性设了。提取器记录
  `[open,refpage='X',...]` 里的 X；缺这个属性的块会导致 `{refpage}`
  替换静默失败，VUID 被丢弃。

## search 没结果

- `--mode bm25`：先确认 `<shard>/bm25/docs.json` 存在。`vkquery index
  list` 列出 shard manifest，含 `bm25: true/false`。
- `--mode embed`：需要 `cargo build --features embed` **构建 shard
  时** 启用——shard 建完后再装 embed feature 是没用的。重建：
  `cargo run --features embed -- index build --tag <tag> --force`。
- `--mode hybrid`：编译期就要求有 `embed` feature。`--no-default-features`
  + 没加 `embed` 时这个 mode 不可用，CLI 会直接报错。

## `git cat-file --batch` 在 Windows 上卡死

`src/git.rs::TagReader` 用 `Stdio::piped()` + 二进制管道；不要改成
text mode。Git for Windows 的 batch 协议在 line-buffered 模式下行为
不一致。

## shard 构建很慢（>30s）

可能原因，按概率排：

- **第一次构建该 tag**，包含 ~5s 的 `git clone --filter=blob:none` +
  按需 blob fetch。设 `VKQUERY_DOCS_PATH` 指向已有的本地完整 clone 可
  跳过。
- **BM25 构建**：tokenization 主导，~1–2s。
- **embedding 构建**：编码 ~27k 条文本。CPU BERT 在 Windows 无 MKL
  时 ~32 vec/30s，全量 HEAD 要 ~7 小时。开发期间用环境变量：
  - `VKQUERY_SKIP_EMBED=1`：完全跳过 embedding，BM25 仍然建。
  - `VKQUERY_EMBED_LIMIT=N`：仅 embed 前 N 条文本，用于 smoke 测试。
  生产场景换用 GPU feature：`--features cuda` / `--features mkl` /
  `--features metal` / `--features accelerate`。
- 都不是的话：检查是不是开了 `RUSTFLAGS="-D warnings"` 又遇到了大量
  `dead_code` 警告，cargo 在 stderr 打 warning 也会拉慢 wall time。

## CI 在 Windows 上跑得格外慢

`tests/parity_bm25.rs` 在冷 CI runner 上可能 5+ min：

- Vulkan-Docs 是 `--depth=1 --filter=blob:none` 克隆，`git cat-file
  --batch` 按需 lazy-fetch blob，每个 blob 一次 HTTP RTT。
- Windows runner 的磁盘 I/O 慢于 Linux runner。
- shard 构建占主导（注册表解析 + BM25 corpus + 全量 VUIDs）。

`.github/workflows/ci.yml` 已设 `timeout-minutes: 30` 兜底。如果实际
体验太慢，下一步可以把这条测试 `#[ignore]` 后单独安排 nightly job。

## `cargo build --features embed` 编译失败

可能原因：

- **candle 版本冲突**——上游某次 patch 升级（如 0.8.4 → 0.8.5）可
  能引入 trait bound 变化。锁定 `Cargo.lock` 一般可避免；如果你刚
  执行了 `cargo update`，回滚 lock 或固定 minor 版本。
- **`half::bf16: SampleUniform` 报错**——历史问题，candle 0.7 与
  `rand 0.9` 不兼容。我们已经升级到 candle 0.8 解决；如果回归请检查
  `Cargo.toml` 里 candle 版本不要降回 0.7。
- **`RUSTFLAGS="-D warnings"` 把 candle 内的 dead_code / unused
  警告升成 error**——这种情况下不要去改 candle 源码，而是临时用
  `RUSTFLAGS=""` 构建。CI 里的 `RUSTFLAGS` 设置只针对我们的代码，
  candle 依赖应该已经走 `--cap-lints` 屏蔽其内部警告；如果失败，可
  能 cargo 行为变了，开 issue。
- **`tokenizers` 加载错误**——`tokenizers 0.20` 需要 `onig` feature
  对应的系统 oniguruma 库，但我们在 `Cargo.toml` 已固定
  `default-features = false, features = ["onig"]`，纯 Rust 实现。
  如果还报错，检查是否环境里有冲突的 native `oniguruma`。

## 模型下载失败

第一次 `search --mode embed`（或 build shard with embed）会从
HuggingFace 下载 `BAAI/bge-small-en-v1.5`（~130MB）到
`<dirs::cache_dir>/vkquery/models/BAAI--bge-small-en-v1.5/`。

可能的失败模式：

- **网络受限**——`ureq` 默认走系统证书；公司网络若做 TLS MITM 需要
  把根证书装进系统 trust store。Rust 端不读 `REQUESTS_CA_BUNDLE` 这类
  环境变量。
- **磁盘空间不足**——下载需要约 200MB（原始下载 + 解压）。
- **`%LOCALAPPDATA%` 权限问题**——某些受管 Windows 环境会阻止往
  `AppData\Local` 写大文件。设 `XDG_CACHE_HOME=<path>` 强制改路径。

下载完成后模型常驻；后续启动直接 mmap，~200ms 就绪。

## GPU feature 选错触发 candle 构建错误

`Cargo.toml` 的 `cuda` / `cudnn` / `mkl` / `accelerate` / `metal` 是
**互斥**的：

```bash
cargo build --features "cuda mkl"          # 会爆
```

candle 的 backend 选择在编译期完成，无法运行时切换。错误信息通常来自
`candle-core` 内部宏，不太友好。**选一个**，按平台：

| 平台 | 推荐 |
|---|---|
| Linux + NVIDIA | `cuda` 或 `cudnn` |
| Linux + Intel | `mkl` |
| Windows + NVIDIA | `cuda` |
| Windows + Intel | `mkl` |
| macOS | `metal` 或 `accelerate` |

裸 `--features embed` 走纯 Rust CPU，最慢但所有平台都行。

## 「No module / library found」类错误

Rust 端没有 Python 那种运行时模块查找，绝大多数缺依赖会在编译时报错。
如果你看到运行时 dynamic-library load 错误，通常是：

- **CUDA Toolkit 没装或 PATH 没设**（`cuda` feature）——
  Linux 上 `nvidia-smi` 能跑，但 `nvcc --version` 报 not found，说明
  driver 装了但 toolkit 没装。
- **Intel MKL 运行时缺失**——装 Intel oneAPI Base Toolkit。Linux
  要 source 一下 `setvars.sh`，Windows 要把 oneAPI 的 redist 路径
  加进 PATH。
- **cuDNN 缺失但启用了 `cudnn` feature**——从 NVIDIA 官网下载 cuDNN
  的 zip / deb，把 `.so` / `.dll` 放到 CUDA Toolkit 同一个 lib 目录。

## 测试在本地能过，CI 上挂

可能原因：

- **`parses_v1_3_250_vk_xml` / `parses_v1_0_40_core_vk_xml`** 在 CI 上
  自动 skip（`--depth=1` 没拉到这些 tag）。这是预期行为，不算挂。
- **`bm25_top5_overlaps_python_on_canonical_queries`** 阈值是 ≥2/5
  overlap + 5% top-1 score drift。如果上游 Vulkan-Docs 变了 prose 内容
  导致 BM25 score 漂移，本地 fixture 也要重生成：
  ```powershell
  python C:\dev\vkquery-rs\target\dump_bm25_top5.py
  ```
  然后提交更新后的 `tests/fixtures/bm25_top5_head.json`。
- **`build_shard_end_to_end_head` 测试 timeout**——CI 上 `timeout-minutes:
  30` 兜底。如果真的需要 30+ min 才能跑完，看 Vulkan-Docs 是不是膨胀
  得不正常了，或者 CI runner 性能短暂下降。

## Shard 在 Vulkan-Docs 提交后过期

Cache 失效**只**看 `xml/vk.xml` 的 content-hash。如果 chapters/ 改了
但 vk.xml 没改，VUIDs/prose 会陈旧。

缓解：

- `vkquery index build --tag HEAD --force` 显式重建。
- 未来计划：把 chapters/ 也算进 shard key（追踪在
  `architecture.md` 的「out of scope」段）。
