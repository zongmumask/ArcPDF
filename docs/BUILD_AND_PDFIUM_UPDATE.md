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

### 2.1 同步源码

```bash
cd <ArcPDF repo>
python3 scripts/build-pdfium sync
```

行为：

- 若 `build/depot_tools` 不存在，克隆 depot_tools。
- 若 `build/.gclient` 不存在，按 `pdfium-revision.toml` 创建。
- 用 gclient 把 PDFium 同步到 manifest 固定 revision。
- 源码落在 `build/pdfium/`，该目录 gitignored。

说明：

- `sync` 使用 `--force`，因为源码工作区是 disposable 的；任何人工修改必须 patch 化到 `pdfium/patches/`。
- 如果本地已有 `build/pdfium`，sync 会把工作区更新到当前 revision。

### 2.2 构建 macOS arm64

```bash
python3 scripts/build-pdfium --target macos_arm64
```

产物：

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

### 4.1 选择新 revision

查询远端 HEAD：

```bash
git ls-remote https://pdfium.googlesource.com/pdfium.git HEAD
```

或直接指定某个 40-hex SHA。

### 4.2 更新 manifest

自动追最新 HEAD：

```bash
python3 scripts/build-pdfium update-revision --latest
```

指定 revision：

```bash
python3 scripts/build-pdfium update-revision --revision <40-hex-sha>
```

### 4.3 同步新源码

```bash
python3 scripts/build-pdfium sync
```

### 4.4 检查 patch

如果 `pdfium/patches/` 有 patch：

```bash
git -C build/pdfium apply --check pdfium/patches/<patch>
```

脚本会在 build 时自动执行 `git apply --check` 和应用。patch 失败必须修复，不允许把修改留在本地源码树里。

### 4.5 重新构建产物

```bash
python3 scripts/build-pdfium --target macos_arm64
```

如果还需要 x86_64：

```bash
python3 scripts/build-pdfium --target macos_x86_64
```

### 4.6 验证

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

### 6.1 `PDFium checkout missing`

```text
PDFium checkout missing at .../build/pdfium. Run: scripts/build-pdfium sync
```

执行：

```bash
python3 scripts/build-pdfium sync
```

### 6.2 `PDFium revision mismatch`

manifest revision 与本地源码 HEAD 不一致。执行：

```bash
python3 scripts/build-pdfium sync
```

### 6.3 gclient `gsutil` lock error

如果多个 gclient 同时运行，可能锁冲突。确认无残留进程后重试：

```bash
python3 scripts/build-pdfium sync
```

### 6.4 Metal shader 编译失败

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
