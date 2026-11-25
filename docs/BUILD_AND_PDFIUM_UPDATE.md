# Arc PDF 新电脑上手、PDFium 构建与定期更新指南

> 适用范围：当前 Rust + GPUI 工程，macOS 首版编译/运行/验证。
> 目标：新电脑拉取代码后，能按固定流程装环境、同步 PDFium、构建产物、跑 App；并支持每约两个月升级一次 PDFium。

---

## 0. 核心策略

本工程采用 **预构建 PDFium 内核 + 消费者侧扩展源码** 的策略：

- PDFium 核心不随项目源码入库，不跟项目一起 git 提交。
- PDFium 源码放在 gitignored 的本地工作区，定期或按需更新。
- `pdfium/pdfium-revision.toml` 固定 PDFium revision 与 GN 参数，是构建的唯一真相。
- 构建产物（`artifacts/`）也 gitignored，由本地/CI 构建生成。
- 需要长期保留的 PDFium 源码修改必须放进 `pdfium/patches/`，不能留在本地源码工作树。
- 当前首版只在 macOS arm64 构建与验证；Windows/Linux/Android/iOS 已在 target matrix 中预留。

这个设计与 ArcPDFium 的“预构建库 + 扩展源码”思想一致，但不绑定 CocoaPods/XCFramework。

---

## 0.1 扩展源码与 PDFium patch 的区别

这里必须区分两类完全不同的代码，避免混淆。

### 扩展源码（Arc 自己的代码，不是 patch）

扩展源码是我们自己维护的 C/C++/Rust 代码，通常位于：

```text
ffi/arc-pdfium-ext/        # 未来迁移扩展方法的 crate
ffi/arc-pdfium-sys/        # bindgen 生成的 public C API binding
```

它们的特点是：

- 是我们自己的源码，正常进 git、正常参与 workspace/CI。
- 通过 `extern "C"` 暴露类似 `FPDFExt_*`、`FPDF_FFLDrawWithMatrix` 的 C API。
- 调用 PDFium public API，必要时引用 PDFium 内部 C++ 头/helper。
- **不修改 PDFium 源码树。**
- 改扩展源码时不需要重新编译 PDFium 内核，只需要重编扩展 crate。

这正是“预构建内核 + 消费者侧扩展源码”里“扩展源码”的部分。

### PDFium patch（修改上游源码，必须 checked-in）

PDFium patch 指的是**直接修改 PDFium 上游源码/构建文件**的改动，例如：

```text
core/fxge/BUILD.gn
testing/test.gni
core/.../*.cpp
core/.../*.h
```

这些改动不能留在本地 `build/pdfium` 工作树，因为该目录是 gitignored 且 disposable，`scripts/build-pdfium sync --force` 会覆盖它。

所有这类改动必须保存为：

```text
pdfium/patches/0001-xxx.patch
pdfium/patches/0002-xxx.patch
```

并由 `scripts/build-pdfium --target ...` 通过 `git apply --check` 和 `git apply` 自动应用。

### 分类表

| 内容 | 放哪里 | 是否 PDFium patch | 是否需要重编 PDFium |
|---|---|---|---|
| 自己写的 `FPDFExt_*` C API | `ffi/arc-pdfium-ext` | 否 | 否 |
| public API 的 Rust binding | `ffi/arc-pdfium-sys` | 否 | 否 |
| 为兼容新 PDFium 修改我们自己的扩展源码 | `ffi/arc-pdfium-ext` | 否 | 否 |
| 修改 PDFium `BUILD.gn` | `pdfium/patches/*.patch` | 是 | 是 |
| 修改 PDFium `core/.../*.cpp/.h` | `pdfium/patches/*.patch` | 是 | 是 |
| 修改 PDFium iOS GN/构建配置 | `pdfium/patches/*.patch` | 是 | 是 |

当前状态：`pdfium/patches/` 为空，macOS arm64 构建不需要任何 PDFium patch。ArcPDFium 现有的 iOS 相关修改只有在未来做 iOS 构建时才需要 patch 化。

---

## 1. 新电脑环境搭建

### 1.1 必需工具

macOS 上需要：

- Xcode（含 Command Line Tools）
- Rust toolchain：仓库 `rust-toolchain.toml` 固定为 `1.100.0`
- `git`
- `python3` 3.11+（脚本用 `tomllib`）
- `gn`（需放入 PATH，例如 `/opt/homebrew/bin/gn`）
- `ninja`（例如 Homebrew 安装）
- Xcode Metal Toolchain（GPUI shader 编译需要）

### 1.2 安装步骤

```bash
# 1) Rust（若未安装）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# 安装后确认：rustc --version 应为 1.100.0（rust-toolchain.toml 会自动切换）

# 2) 常用工具
brew install ninja python3

# 3) gn：从已有 PDFium buildtools 或官方构建获取，放入 PATH
#    本工程不在仓库内提交 gn 二进制
cp /path/to/pdfium/buildtools/mac/gn /opt/homebrew/bin/gn
chmod +x /opt/homebrew/bin/gn
gn --version

# 4) GPUI 所需 Metal Toolchain
xcodebuild -downloadComponent MetalToolchain

# 5) 检查工具
scripts/setup-dev.sh
```

### 1.3 仓库结构（关键目录）

```text
pdfium/
  pdfium-revision.toml       # revision + target matrix + GN args
  patches/                   # checked-in PDFium patches（当前为空）
scripts/
  build-pdfium               # sync/update-revision/build 入口
  check.sh                   # 本地统一检查
build/                       # gitignored：depot_tools + PDFium 源码工作区
artifacts/                   # gitignored：libpdfium.a + headers + manifest.json
vendor/zed/                  # vendored GPUI 快照（来自 quiet-md 已验证版本）
crates/                      # Rust workspace
```

---

## 2. 首次拉取源码与构建 PDFium

### 2.1 一条命令完成构建

```bash
cd <ArcPDF repo>
scripts/build-pdfium
```

默认等价于：

```bash
scripts/build-pdfium --target macos_arm64
```

脚本会自动完成整个流程：

1. 读取 `pdfium/pdfium-revision.toml`。
2. 若 `build/depot_tools` 不存在，克隆 depot_tools。
3. 若 `build/pdfium` 不存在，或其 HEAD 与 manifest revision 不符，自动 gclient sync。
4. `git apply --check` / 应用 `pdfium/patches/` 中的 patch。
5. `gn gen`。
6. `ninja pdfium`。
7. 复制 `libpdfium.a` 和 public headers。
8. 写入 manifest。

因此日常使用不需要先手动执行 `sync` 再执行 build。`sync` 和 `update-revision` 仍保留为高级命令，但正常开发不必直接调用。

源码落在 `build/pdfium/`，该目录 gitignored、可丢弃、可重建。

说明：

- 内部 gclient sync 使用 `--force`，因为源码工作区是 disposable 的；任何人工修改必须 patch 化到 `pdfium/patches/`。
- 如果本地源码已存在且 revision 正确，构建过程不会联网。

### 2.2 构建产物

```text
artifacts/pdfium/macos_arm64/libpdfium.a
artifacts/pdfium/macos_arm64/include/*.h
artifacts/pdfium/macos_arm64/manifest.json
artifacts/pdfium/macos_arm64/out/            # gn/ninja 中间产物
```

manifest 记录：

- PDFium revision
- GN args
- applied patches
- lib SHA256
- header SHA256
- source worktree dirty 状态

当前已知的 dirty 记录：

```text
D testing/resources/get_page_aaction.pdf
```

这是 gclient checkout 的历史 fixture 不一致，不影响构建，manifest 会显式记录。

### 2.3 构建 macOS x86_64（预留实现）

```bash
python3 scripts/build-pdfium --target macos_x86_64
```

target matrix 里该目标为 implemented，但当前验证基线是 arm64；x86_64 首次构建可能需要处理工具链差异。

### 2.4 其他平台

```bash
python3 scripts/build-pdfium --list-targets
```

当前：

- `macos_arm64`、`macos_x86_64`：implemented
- `windows_*`、`linux_*`、`android_*`、`ios_*`：reserved

reserved 目标会明确失败，避免误以为已支持。

---

## 3. 日常开发与验证

### 3.1 全量检查

```bash
scripts/check.sh
```

包含：

```bash
cargo check --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

### 3.2 命令行渲染 smoke

```bash
cargo run -p arc-pdf-cli -- render \
  --pdf artifacts/test-fixtures/smoke.pdf \
  --page 0 --scale 1.5 \
  --out artifacts/test-fixtures/smoke-page0.png
```

### 3.3 启动 GUI

无参数启动 LibraryWindow：

```bash
cargo run -p arc-pdf-app
```

直接打开 PDF：

```bash
cargo run -p arc-pdf-app -- artifacts/test-fixtures/smoke.pdf
```

GUI 需要 macOS 图形会话与 Metal Toolchain。

---

## 4. 每两个月升级 PDFium 的流程

建议节奏：**默认每两个月一次，遇到安全修复或内部 API 不兼容时立即升级。**

### 4.1 一键升级并构建（推荐）

查询最新 HEAD、更新 manifest、同步源码、重新构建 macOS arm64：

```bash
scripts/build-pdfium --update
```

指定某个 40-hex SHA：

```bash
scripts/build-pdfium --revision <40-hex-sha>
```

这两条命令等价于：

```text
更新 revision → gclient sync → git apply patches → gn gen → ninja → 写 manifest
```

也就是说，用户不需要手动执行 `update-revision` + `sync` + build 三个步骤。

### 4.2 高级手动步骤（通常不需要）

如果需要拆分调试，仍可显式调用：

```bash
scripts/build-pdfium update-revision --revision <40-hex-sha>
scripts/build-pdfium sync
scripts/build-pdfium --target macos_arm64
```

### 4.3 patch 检查

如果 `pdfium/patches/` 有 patch，构建脚本会自动执行：

```bash
git apply --check <patch>
git apply <patch>
```

patch 失败必须修复，不允许把修改留在本地源码树里。

### 4.4 需要同时构建 x86_64

```bash
scripts/build-pdfium --target macos_x86_64
```

注意：如果上一步已经通过 `--update`/`--revision` 更新了 manifest，此命令会使用同一个新 revision。

### 4.5 验证

```bash
scripts/check.sh
cargo run -p arc-pdf-cli -- render --pdf artifacts/test-fixtures/smoke.pdf --page 0 --scale 1.5 --out /tmp/upgraded.png
cargo run -p arc-pdf-app -- artifacts/test-fixtures/smoke.pdf
```

检查：

- manifest revision 是否为新 SHA
- lib SHA256 是否变化
- GUI 是否能打开/渲染/滚动/缩放
- 扩展层（当前尚未迁移）是否不受影响

### 4.7 提交内容

升级时只提交：

```text
pdfium/pdfium-revision.toml
pdfium/patches/（如有变化）
相关 Rust binding 适配代码
文档（如版本矩阵变化）
```

不要提交：

```text
build/
artifacts/
target/
```

它们已被 gitignore。

---

## 5. 为什么这个方案合理

- PDFium 源码 3–4 GB、gn/ninja 中间产物体积大，跟随项目提交会污染仓库并拖慢 clone。
- 两个月一次升级能与上游保持稳定距离，又不会让维护者永远追移动靶。
- manifest 固定 revision，保证同一提交下本地与 CI 构建同源。
- 源码工作区 disposable；patch checked-in，避免“开发机能编、新电脑编不过”。
- 预构建内核低频更新，扩展源码高频迭代，兼顾构建速度与扩展性。

---

## 6. 常见问题

### 6.1 源码缺失或 revision 不匹配

正常 `scripts/build-pdfium` 会自动处理。只有手动拆分流程时才会看到：

```text
PDFium checkout missing at .../build/pdfium. Run: scripts/build-pdfium sync
PDFium revision mismatch
```

执行：

```bash
scripts/build-pdfium
```

或直接重新跑一次构建即可；脚本会在需要时自动 sync。

### 6.2 gclient `gsutil` lock error

如果多个 gclient 同时运行，可能锁冲突。确认无残留进程后重试：

```bash
scripts/build-pdfium
```

不需要单独 sync。

### 6.3 Metal shader 编译失败

```text
missing Metal Toolchain; use: xcodebuild -downloadComponent MetalToolchain
```

执行：

```bash
xcodebuild -downloadComponent MetalToolchain
```

### 6.5 manifest 里 `source_worktree_dirty` 非空

当前已知：`D testing/resources/get_page_aaction.pdf`，来自 gclient 的历史 fixture 不一致，不影响构建。

原则：

- 若 dirty 文件会影响编译头/库，必须 patch 化并重新构建。
- 若只是无关 fixture，可记录在 manifest 中，不提交源码工作区。
