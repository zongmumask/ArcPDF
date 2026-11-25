# Arc PDF Rust + GPUI 实施计划（macOS 首版）

> 文档日期：2026-11-24
> 性质：实施计划。目标不是翻译现有 Swift 工程，而是从零构建一个可持续演进的 Rust + GPUI PDF 应用。
>
> 与架构文档的关系：
>
> - 架构与源码分析见 `docs/ARC_PDF_RENDERING_ARCHITECTURE_AND_RUST_MIGRATION.md`。
> - 本计划定义环境、工程实践、文档体系、开发规范、CI/CD、测试、发布、PDFium 构建和窗口级交付路线。
> - 本计划中的 Rust 目标是**未来系统**；文中引用的 Swift 路径是现有行为参照，不是迁移对象。
>
> 标注约定：
>
> - **【现状事实】**：现有仓库中可直接验证的实现。
> - **【目标】**：Rust 首版要实现的交付结果。
> - **【实施】**：建议的具体执行动作。
> - **【验收】**：可检查、可运行的完成标准。
> - **【预留】**：macOS 首版不做，但设计上必须保留的扩展点。

---

## 0. 项目目标与非目标

### 0.1 唯一主验收场景

**【目标】** 首版最终验收不是“某个渲染函数能跑”，而是完整应用链路：

```text
启动 Rust App
  → 显示 LibraryWindow（Rust/GPUI 窗口）
  → 从 Library 打开一个 PDF 文件（文件选择/拖拽/命令行路径至少一种）
  → LibraryWindow 关闭或隐藏
  → 显示 PDFWindow（Rust/GPUI 窗口）
  → PDF 页面被渲染出来（低清 Preview + 高清 Tile）
  → 可以滚动（连续多页，内容随视口更新）
  → 可以缩放（鼠标/触控板/快捷键至少一种，页面清晰度随之变化）
```

**【验收】** 上述链路在 macOS arm64 与 x86_64（Rosetta 或原生均可）上可重复执行；无崩溃；渲染内容与 Swift 基线视觉一致或差异在记录容差内。

### 0.2 首版范围

**【目标】** 首版必须做：

1. Rust workspace 与工具链。
2. PDFium 构建系统：先只完整支持 macOS arm64/x86_64，但配置与目录结构预留 Windows/Linux。
3. PDFium public C API binding；扩展机制按 A/B/C 分层预留。
4. `arc-pdf-core`：文档模型、页几何、generation、viewport。
5. `arc-pdf-engine`：进程级 PDFium actor。
6. `arc-pdf-render`：preview + tile + 缓存 + 调度（无 GPUI）。
7. `arc-pdf-ui`：GPUI `LibraryWindowController` 与 `PDFWindowController` 等价窗口。
8. `arc-pdf-platform`：文件选择、路径访问、内存压力等平台 trait。
9. 工程体系：文档、规范、CI/CD、测试、回归、fuzz、benchmark、release。

**【非目标】** 首版不做：

- 批注编辑、表单编辑、文本编辑、保存写回。
- 云盘账号、AI、OCR、水印、页面管理、导出。
- PDFium 全量扩展移植。
- Windows/Linux UI 发布。
- App Store 提交。

### 0.3 现有窗口职责参照

**【现状事实】** Swift App 的窗口入口：

- `Quill/Library/Controllers/LibraryWindowController.swift:16`：`final class LibraryWindowController: NSWindowController`，singleton `shared`；持有 `LibrarySplitViewController`；有 toolbar、文档列表、排序/视图切换；`:26` 定义 `onOpenDocument: ((URL) -> Void)?`；`:208-210` 在文档选择时回调；`:98-106` 的 `show()` 刷新内容、重建 toolbar、置前窗口。
- `Quill/AppDelegate/AppDelegate.swift:192-199`：`library.onOpenDocument` 收到 URL 后调用 `showMainWindow(url)`，随后关闭 library window。
- `Quill/Controllers/PDFWindowController.swift:13`：`class PDFWindowController: NSWindowController`；`:15-21` 以 `PDFWorkspaceSplitViewController` 为 window root；`:184-192` 的 `displayPDFController(_:)` 用 URL 打开或选中已有 tab；`:223-283` 的 `openDocumentInNewTab` 创建 `Document`、调用 `openWithPassword`、成功后构造 `PDFViewController` 加入 tab。
- `Quill/AppDelegate/AppDelegate.swift:154-175`：没有已有 PDF 窗口时创建 `PDFWindowController` 并 `showWindow(nil)`；最后一个 PDF 窗口关闭时重新显示 Library。
- `Quill/PDFKit/View/PDFView.swift:162-165,191-196,308-333`： magnification、fit width、zoom 手势与 clip bounds 通知。

**【目标】** Rust 首版不复制三栏 split、tab、toolbar 全量功能，但必须保留窗口状态机：

```text
AppLaunch
  ├─ if launched with PDF path → open PDFWindow directly (Library 不显示)
  └─ else → LibraryWindow
        └─ user opens file → close/hide LibraryWindow → open PDFWindow
              └─ PDFWindow closed → show LibraryWindow again
```

---

## 1. 环境搭建

### 1.1 开发者机器

**【实施】** macOS 开发环境：

- macOS 14+（建议与现有 CI `macos-15` 对齐）。
- Xcode 16+/Command Line Tools（需 `xcodebuild`、`lipo`、`codesign`）。
- Rust toolchain：由 `rust-toolchain.toml` pin，建议与 quiet-md 一致的显式版本策略；至少包含 `clippy`。
- Homebrew 依赖：`gn`、`ninja`、`python3`、`pkg-config`（如需要）。
- PDFium 构建依赖：depot_tools、gclient、CIPD、Python、Git。脚本不得假设 `/opt/homebrew/bin/ninja` 是唯一路径，需按 `command -v` 回退。
- CocoaPods 不需要；Rust 项目不再依赖 CocoaPods 交付。

**【验收】** 新机器按 `docs/BUILD.md` 从零执行：

```bash
scripts/setup-dev.sh
scripts/check.sh
```

均通过，无手工复制头文件/源码步骤。

### 1.2 仓库初始化

**【实施】** 当前目录 `/Users/danielhu/Projects/ArcPDF` 建立 workspace：

```text
Cargo.toml
rust-toolchain.toml
Cargo.lock
crates/
  arc-pdf-core/
  arc-pdf-engine/
  arc-pdf-render/
  arc-pdf-ui/
  arc-pdf-platform/
ffi/
  arc-pdfium-sys/
  arc-pdfium-ext/
pdfium/
  pdfium-revision.toml
  patches/
  build.rs
tests/
  fixtures/
tools/
  harness/
  perf/
docs/
  ARCHITECTURE.md
  RENDERING.md
  PDFIUM.md
  TESTING.md
  BUILD.md
  DEVELOPMENT.md
  RELEASE.md
.github/workflows/
```

**【验收】** `cargo metadata --locked` 成功；workspace 成员清晰；`arc-pdf-core` 不依赖 GPUI/PDFium（可用 `cargo tree -p arc-pdf-core` 检查）。

### 1.3 版本与工具链 pin

**【实施】** 类似 quiet-md：

- `rust-toolchain.toml` pin channel 与 `clippy`。
- `Cargo.lock` 提交并 CI 使用 `--locked`。
- `pdfium/pdfium-revision.toml` 固定 PDFium SHA、HarfBuzz SHA、patch 列表、GN args。
- 构建产物 manifest 记录：PDFium SHA、头文件 hash、静态库 hash、目标平台、patch 结果。

**【验收】** 同一 revision 在两台机器上构建得到相同 manifest 字段；版本信息可在 App About/日志中查看。

---

## 2. 工程实践与模块职责

### 2.1 Workspace 分层

**【实施】** 依赖方向严格单向：

```text
arc-pdf-core     无 GPUI / 无 PDFium / 无平台 API
    ↑
arc-pdf-engine   依赖 arc-pdf-core + arc-pdfium-sys；PDFium actor
    ↑
arc-pdf-render   依赖 arc-pdf-core + arc-pdf-engine；调度、缓存、preview/tile；无 GPUI
    ↑
arc-pdf-ui       依赖 GPUI + arc-pdf-render；窗口、视图、Element
    ↑
ArcPdfApp        根二进制；启动、菜单、窗口管理

arc-pdf-platform 提供 trait；各平台实现由 target gate 注入
```

**【验收】**

- `cargo test -p arc-pdf-core -p arc-pdf-render` 可在无窗口环境执行。
- `arc-pdf-ui` 之外不允许出现 PDFium C 类型。
- `arc-pdf-render` 之外不允许直接创建 GPUI `RenderImage`。

### 2.2 Rust / GPUI 职责

**【实施】**

| 模块 | 负责 | 不负责 |
|---|---|---|
| `arc-pdf-core` | DocumentId/PageId、PageGeometry、generation、viewport、命令/事件语义 | 渲染、线程、GPUI、PDFium |
| `arc-pdf-engine` | PDFium init、句柄、渲染请求、bitmap buffer、FIFO close | UI、缓存策略、滚动 |
| `arc-pdf-render` | visible/prefetch 集合、tile key、scale bucket、调度、去重、取消、缓存 | GPUI 绘制、PDFium 句柄 |
| `arc-pdf-ui` | GPUI Window/Entity/Element、paint_image、输入事件 | PDFium 调用、复杂缓存策略 |
| `arc-pdf-platform` | 文件对话框、文件访问、内存压力、系统字体 | PDF 逻辑 |

**【验收】** `cargo clippy --workspace --all-targets -- -D warnings` 通过；模块边界由 code review checklist 检查。

### 2.3 开发规范

**【实施】** 建立 `docs/DEVELOPMENT.md` 与 `AGENTS.md`/`CLAUDE.md`（内容同步）：

1. 所有渲染输出变化必须带 generation/key。
2. 所有 UI 状态更新只在 GPUI foreground；后台只回传数据。
3. 新增 structural change 必须推进 generation，并加入 fuzz 用例。
4. 渲染策略修改必须更新 `docs/RENDERING.md` 与 golden baseline。
5. PDFium 升级必须更新 `pdfium-revision.toml`、扩展分层清单、回归报告。
6. 不运行 rustfmt（若项目采用 quiet-md 紧凑风格）；如决定采用 rustfmt，则必须在全仓库统一并由 CI 强制。
7. 测试/脚本不得使用用户真实文件目录；使用临时目录和 fixture。
8. 提交前运行 `scripts/check.sh`。

**【验收】** 新贡献者按文档可独立完成一次“修改 → 检查 → 测试 → 提交”流程。

---

## 3. 文档体系

**【实施】** 初始文档集：

| 文档 | 内容 | 完成阶段 |
|---|---|---|
| `README.md` | 项目定位、快速开始、当前支持平台与限制 | Phase 0 |
| `docs/ARCHITECTURE.md` | workspace、模块边界、事件流、窗口状态机 | Phase 0 |
| `docs/RENDERING.md` | preview/tile、scale bucket、visible/prefetch、generation、cache | Phase 3 |
| `docs/PDFIUM.md` | revision、GN args、patch、A/B/C 扩展、ABI manifest | Phase 2 |
| `docs/TESTING.md` | unit/golden/fuzz/harness/benchmark/self-test | Phase 1 |
| `docs/BUILD.md` | 环境、本地构建、macOS 双架构、后续平台预留 | Phase 0 |
| `docs/DEVELOPMENT.md` | 开发流程、代码规范、review checklist | Phase 0 |
| `docs/RELEASE.md` | 版本、tag、打包、签名、发布、回滚 | Phase 7 |
| `docs/PERFORMANCE.md` | benchmark 方法、环境、结果 | Phase 6 |
| `AGENTS.md` / `CLAUDE.md` | AI agent 指引，保持同步 | Phase 0 |

**【验收】** 每个阶段结束时，相关文档与代码/配置一致；CI 中可加文档链接检查。

---

## 4. PDFium 集成构建（macOS 首版，多平台预留）

### 4.1 设计原则

**【实施】** 保留 ArcPDFium 的核心策略：**预构建 PDFium 内核 + 消费者侧扩展源码编译**；不绑定 CocoaPods、XCFramework 或任何 Apple-only 打包方式。

- PDFium 内核低频预构建。
- 扩展源码消费者侧编译，快速迭代。
- 扩展对外保持 public-style C ABI。
- B/C 层扩展与 PDFium SHA 绑定。

**【首版基准】** macOS 编译与验证：先打通 `macos_arm64` 与 `macos_x86_64` 的 GN/ninja 构建、public header 导出、manifest 校验和 Rust binding 编译。

**【预留】** Windows、Linux、Android、iOS 从第一天进入目标矩阵设计：

- `pdfium-revision.toml` 中声明全部 target 的 `target_os/target_cpu/target_environment` 与必要 GN args。
- 构建脚本支持 `--target <name>`，macOS 首版只实现并验证 macOS targets，其他 targets 输出明确的“not implemented yet”。
- 产物目录按 `artifacts/<platform>/<arch>/<environment>/` 划分，不假设 XCFramework slice。
- 扩展 crate 的 ABI manifest 与平台无关；各平台只消费对应静态库与头文件。

### 4.2 构建流程

**【实施】** `scripts/build-pdfium`：

1. 读取 `pdfium/pdfium-revision.toml`。
2. 引导 depot_tools/CIPD，校验工具版本。
3. `gclient config` + `gclient sync`，检出固定 PDFium SHA。
4. `git apply --check` 应用 `pdfium/patches/*.patch`。
5. 按目标生成 GN args：
   - macOS arm64：`target_os="mac" target_cpu="arm64"`。
   - macOS x86_64：`target_os="mac" target_cpu="x64"`。
   - Windows x64/arm64：`target_os="win" target_cpu="x64|arm64"`（预留）。
   - Linux x64/arm64：`target_os="linux" target_cpu="x64|arm64"`（预留）。
   - Android arm64/armv7：`target_os="android" target_cpu="arm64|arm"`（预留）。
   - iOS device/simulator：`target_os="ios" target_cpu="arm64" target_environment="device|simulator"`（预留）。
6. `ninja -C out/<target> pdfium`。
7. 导出 public headers 到构建输出目录。
8. 计算并写入 manifest：
   - pdfium_sha / harfbuzz_sha
   - headers_sha256
   - libpdfium_sha256 per target
   - applied patches
   - GN args
9. macOS 可选 `lipo -create` 生成 local fat lib；其他平台不假设 Apple 打包格式，App/Rust 构建按目标选择对应静态库。

**【验收】**

- `scripts/build-pdfium --target mac-arm64` 与 `--target mac-x86_64` 可独立构建。
- 修改 revision 或 patch 后 manifest 变化；未变时二次构建 manifest 一致。
- `arc-pdfium-sys` 的 bindgen 输入来自该 revision 的 `public/`，hash 不匹配即构建失败。

### 4.3 Binding 与扩展

**【实施】**

- `ffi/arc-pdfium-sys`：bindgen public headers；C 类型只在 sys crate 内部。
- `ffi/arc-pdfium-ext`：按 A/B/C 分层：
  - A：public-only C shim，稳定 ABI。
  - B：内部 helper 依赖，声明 `required_pdfium_sha` 或兼容区间。
  - C：内部 ABI/私有成员依赖，绑定单一 SHA，需要显式启用 feature。
- 扩展 manifest：`abi_level`、`pdfium_sha`、`symbols`、`source_files`。
- 构建期检查：`clang++ -c` + link smoke + `nm` 导出/重复符号检查。

**【验收】**

- Phase 2 仅需 public API 渲染普通 PDF；不启用 C 层扩展也能跑通主链。
- 升级 PDFium SHA 时，B/C 层不匹配会在 build/CI 阶段失败，而非运行时崩溃。

**【说明】** 这不是否定现有“预构建库 + 扩展源码”模型，而是把它工程化：保留快速扩展和 public-style C ABI，增加版本与验证边界。

---

## 5. CI/CD 与 GitHub Actions（低优先级）

> **优先级：低**。Phase 0–4 先以本地 `scripts/check.sh` 和手工验收为主；GitHub Actions、复杂缓存与发布流水线在核心链路（Library → PDF → 渲染 → 滚动 → 缩放）跑通后补齐。本章保留完整设计，避免后续返工。

### 5.1 本地检查入口

**【实施】** `scripts/check.sh` → `scripts/check.py`：

```bash
cargo test --release --workspace --locked
cargo clippy --release --workspace --locked --all-targets -- -D warnings
cargo clippy --release --workspace --locked --all-targets --features devtools -- -D warnings
python3 tools/check-pdfium-manifest.py
python3 tools/check-golden.py
```

**【验收】** macOS/Windows/Linux 上同一脚本可运行（Windows 可用 `python scripts/check.py`）。

### 5.2 GitHub Actions

**【实施】** `.github/workflows/ci.yml`：

| Job | Runner | 内容 |
|---|---|---|
| `check-macos` | macos-15 | build、test、clippy、feature matrix、golden render、PDFium manifest |
| `pdfium-macos` | macos-15 | 构建 arm64/x86_64 静态库；缓存 depot_tools/out；上传 manifest |
| `windows-check` | windows-2022 | 仅 cargo check/test/clippy + PDFium 构建脚本参数 dry-run；UI 暂不发布 |
| `linux-check` | ubuntu-22.04 | 仅 cargo check/test/clippy + 必要系统依赖；UI 暂不发布 |
| `release` | macos-15 | tag 触发：dist build、签名、打包、self-test、上传产物 |

**【实施】** Actions 使用 commit SHA pin；docs-only PR 可跳过昂贵构建（参照 quiet-md `.github/workflows/ci.yml:8-12`）。

**【验收】** PR 必须绿；PDFium revision 变更触发完整构建；普通代码变更可复用缓存。

### 5.3 缓存策略

**【实施】**

- 缓存 `~/.cargo` 与 `target/`（按 lockfile hash）。
- 缓存 depot_tools 与 PDFium git object，但 revision/patch 变化时失效。
- 缓存 gn/ninja out 目录需包含 revision + GN args hash。

**【验收】** 二次 CI 构建时间显著低于首次；缓存 key 变更可解释。

---

## 6. 自动化测试、回归、Fuzz、Benchmark（低优先级）

> **优先级：低**。Phase 0–4 只要求最小单元测试和 golden smoke；完整 golden 矩阵、UI harness、fuzz、benchmark 与 release self-test 在主验收链路通过后再系统化补齐。注意：低优先级不等于取消——第 10 章最终验收中的检查项仍需完成。

### 6.1 单元测试

**【实施】** 在 `arc-pdf-core`/`arc-pdf-render` 覆盖：

- page rect ∩ extendedVisibleRect / visibleRect。
- 128pt tile 网格切分、边界格、`ceil` pixel size。
- scale bucket 量化和 zoom settle。
- cropBoxOffset 方向。
- cache key 稳定性（generation/theme/flags/role）。
- cancel/duplicate/stale result 状态机。

**【验收】** `cargo test -p arc-pdf-core -p arc-pdf-render` 无窗口可跑。

### 6.2 渲染回归

**【实施】**

- `tests/fixtures/` 放固定 PDF 样本与 baseline PNG：
  - 单页文本、多页、旋转页、cropBox ≠ mediaBox、透明图、嵌入字体、表单/批注（Phase 5）、超高分辨率页。
- CLI `arc-pdf-cli render` 在固定 bucket 下输出 PNG。
- `tools/check-golden.py` 做逐像素比较并记录容差。

**【验收】** 渲染策略变更前后 golden 图差异符合预期；纯重构必须像素一致。

### 6.3 UI harness

**【实施】**

- 根二进制加 `--features devtools` 时启用脚本驱动（发布构建不含）。
- 脚本命令参考 quiet-md：`wait/click/drag/wheel/pinch/key/shot/bench/quit`。
- 场景：
  1. 启动显示 LibraryWindow。
  2. 打开 fixture PDF。
  3. PDFWindow 出现并渲染。
  4. 滚动 20 次，截图。
  5. pinch/键盘缩放，截图。
  6. 关闭 PDFWindow 回到 LibraryWindow。

**【验收】** `tools/harness/run.py` 可在临时目录运行；`compare.py` 可做新旧二进制逐像素对比。

### 6.4 Fuzz

**【实施】** 确定性随机 viewport fuzz：

- 随机 scroll/zoom/page count change/theme change/token cancel。
- 不变量：
  - 无同一 RenderKey 多个活跃请求。
  - cache bytes ≤ budget。
  - generation 过期结果不上屏。
  - 离开 prefetch 区后任务被取消。
- `ARCPDF_FUZZ_SEED/SEEDS/STEPS` 控制重放。

**【验收】** CI 默认 40 seeds，夜间/本地 1000 seeds。

### 6.5 Benchmark

**【实施】**

- `tools/perf/render_bench.py` 调用 devtools harness 的 `bench` 命令。
- 指标：
  - cold open 到首帧 preview 时间。
  - scroll frame P50/P95。
  - zoom settle 后 tile fully-rendered 时间。
  - cache hit rate、in-flight count、render ms by role。
  - memory footprint（macOS `footprint`）。
- 结果写入 `docs/PERFORMANCE.md`，含机器、OS、窗口尺寸、PDF fixture。

**【验收】** Phase 6 输出可复现 benchmark 报告。

### 6.6 Release self-test

**【实施】** 发布二进制支持 `--self-test <empty dir>`：

- 临时目录打开 fixture PDF。
- 检查 Library/PDF 窗口创建、渲染非空、滚动/缩放后截图非空。
- 两阶段启动检查退出后状态。
- 不依赖 devtools feature。

**【验收】** 发布包自测退出码 0；报告包含每步耗时与失败截图。

---

## 7. 分阶段路线

### Phase 0：仓库、文档、环境与最小窗口

| 项 | 内容 |
|---|---|
| 目标 | 工程骨架与最小 GPUI 窗口 |
| 范围 | Workspace、toolchain、check 脚本、文档初版、GPUI hello window |
| 实施 | 建立目录；`scripts/setup-dev.sh`；`scripts/check.sh`；根二进制启动一个 GPUI 窗口 |
| 交付 | 可 clone 后一键检查的仓库 |
| 验证 | `scripts/check.sh` 通过；文档齐备 |
| 风险 | toolchain/GPUI 版本不稳 |
| 完成标准 | 三平台 `cargo check --workspace --locked` 通过；macOS 可显示空窗口 |

### Phase 1：PDFium public binding 与 CLI 渲染

| 项 | 内容 |
|---|---|
| 目标 | Rust 能打开 PDF 并渲染单页 PNG |
| 范围 | PDFium 构建脚本、revision manifest、`arc-pdfium-sys`、`arc-pdf-engine`、CLI |
| 实施 | `FPDF_InitLibraryWithConfig`；`FPDF_LoadDocument`；`FPDF_GetPageSizeByIndex`；`FPDF_LoadPage`；`FPDF_RenderPageBitmapWithMatrix` |
| 交付 | `arc-pdf-cli render --pdf fixtures/simple.pdf --page 0 --out /tmp/page.png` |
| 验证 | 与 Swift 基线做像素比较；manifest 校验 |
| 风险 | binding/矩阵方向错误 |
| 完成标准 | 普通 PDF 页渲染正确；无内存错误；PDFium 仅单线程调用 |

### Phase 2：Rendering Core（preview + tile + 缓存，无 UI）

| 项 | 内容 |
|---|---|
| 目标 | 渲染策略完全独立于 GPUI 可测试 |
| 范围 | `arc-pdf-core`、`arc-pdf-render`、调度/缓存/取消/generation |
| 实施 | 128pt tile、scale bucket、visible/prefetch 集合、优先级、in-flight、LRU/TTL |
| 交付 | 调度器单元测试 + fuzz + CLI 批量渲染 |
| 验证 | fuzz 不变量；cache bytes 不越界；无重复/过期结果 |
| 风险 | bucket 选择影响清晰度 |
| 完成标准 | 1000 fuzz seeds 通过；策略文档与实现一致 |

### Phase 3：LibraryWindow 与 PDFWindow 骨架

| 项 | 内容 |
|---|---|
| 目标 | 建立用户要求的窗口流程 |
| 范围 | `arc-pdf-ui`、`arc-pdf-platform`、窗口状态机、文件打开 |
| 实施 | GPUI `LibraryWindow`：显示 PDF 列表/打开按钮；`PDFWindow`：承载 PDF 视图；AppDelegate 等价状态机 |
| 交付 | 从 Library 打开 fixture PDF 后显示 PDFWindow，即使渲染暂为占位 |
| 验证 | harness 场景 1–3、6；窗口关闭后回到 Library |
| 风险 | GPUI 多窗口管理与 platform menu 差异 |
| 完成标准 | Library → PDF → close → Library 流程稳定 |

### Phase 4：PDFWindow 渲染、滚动、缩放（主验收）

| 项 | 内容 |
|---|---|
| 目标 | 达成用户定义的最终效果 |
| 范围 | GPUI PageElement、paint_image、scroll handle、zoom 输入 |
| 实施 | preview 先显；tile 覆盖；滚动更新 visible/prefetch；缩放 bucket 与 settle；键盘/滚轮/pinch 输入 |
| 交付 | 可打开 PDF、渲染、滚动、缩放的 macOS App |
| 验证 | harness 场景 1–6；golden 图；手动验收 |
| 风险 | GPUI Element 生命周期与 cache 交互 |
| 完成标准 | **主验收场景全部通过** |

### Phase 5：扩展能力分层审计与按需接入

| 项 | 内容 |
|---|---|
| 目标 | 在保持快速扩展优势下接入必要能力 |
| 范围 | 22 个扩展 `.cpp` 的 A/B/C 分类；仅接入主链需要的能力 |
| 实施 | public-only shim 优先；B/C 层绑定 PDFium SHA；link/nm 验证 |
| 交付 | 扩展 manifest 与分类表；必要功能的 Rust 调用 |
| 验证 | 扩展层升级测试；符号冲突检查 |
| 风险 | 内部 ABI 漂移 |
| 完成标准 | 每个扩展有明确分类与升级策略；主链不依赖未标注扩展 |

### Phase 6：内存、性能与稳定性

| 项 | 内容 |
|---|---|
| 目标 | 长会话稳定 |
| 范围 | LRU/TTL、GPU texture 回收、mmap、内存压力、1000 页文档 |
| 实施 | 上传后释放 CPU 副本；大 buffer mmap；memory pressure sweep；结构变化 generation |
| 验证 | benchmark + memory report |
| 风险 | 过度淘汰导致白屏 |
| 完成标准 | 内存有上限；10 分钟滚动无泄漏；帧时达标 |

### Phase 7：Release、版本管理与跨平台预留收尾

| 项 | 内容 |
|---|---|
| 目标 | macOS 发布可重复，Windows/Linux 构建路径预留完整 |
| 范围 | dist profile、packaging、签名、self-test、版本号、changelog |
| 实施 | release workflow；版本同步；dSYM；SHA-256；Windows/Linux CI dry-run |
| 交付 | `docs/RELEASE.md`；发布脚本；release self-test |
| 验证 | tag 触发完整流程；发布包 self-test |
| 风险 | 签名/公证凭据配置 |
| 完成标准 | 一次 tag 可产出可验证 macOS 包；Windows/Linux 至少 build/manifest 层通过 |

---

## 8. Release、版本管理与发布

### 8.1 版本号

**【实施】**

- 根 crate、core crate、engine crate 版本号统一。
- `CHANGELOG.md` 与 `docs/releases/<version>.md` 同步。
- 版本来源单一：`Cargo.toml`；打包脚本读取，不手写。
- PDFium revision 与扩展 ABI level 写入发布 manifest。

### 8.2 发布流程

**【实施】** tag `v*` 触发：

1. `cargo build --profile dist --locked`。
2. `scripts/check.sh`。
3. golden render + release self-test。
4. 打包 `.app`/zip，codesign/notarize/staple。
5. dSYM 单独压缩保存。
6. 生成 SHA-256。
7. 上传 artifact 或 release draft。

**【验收】** 发布包在空格/CJK 路径下运行 `--self-test` 通过；签名校验通过；manifest 可追溯 PDFium SHA。

### 8.3 跨平台发布预留

**【预留】** Windows/Linux 不改业务代码即可接入：

- `scripts/build-pdfium --target windows-x86_64` / `linux-x86_64`。
- `arc-pdf-platform` 的 `FileDialog` / `MemoryPressure` / `SystemFonts` trait 实现。
- CI job 先保证 `cargo check --target <triple>` 和 PDFium manifest 构建通过。
- UI 发布延后到 Phase 7 之后。

---

## 9. 风险与依赖

| 风险 | 缓解 |
|---|---|
| GPUI 版本/patch 不稳定 | vendor pin + patch 记录 + `vendor_sync check` |
| PDFium 升级导致 B/C 层扩展失败 | A/B/C 分层 + SHA manifest + CI compile/link/nm |
| Preview/Tile 策略过度复杂 | Phase 2 无 UI 先做状态机与 fuzz |
| 窗口流程与渲染耦合 | Phase 3 先占位窗口，Phase 4 再接渲染 |
| 主验收未定义量化指标 | 用 harness 场景 + golden + 手动验收共同判定 |
| macOS 双架构测试成本 | arm64 原生 + x86_64 Rosetta smoke；release 再做原生交叉 |
| 用户数据风险 | 所有测试用 fixture 与临时目录；不触碰真实文档库 |

---

## 10. 主验收清单（最终）

**【验收】** 以下每一项必须通过：

1. `scripts/check.sh` 全绿。
2. `cargo test --release --workspace --locked` 通过。
3. `cargo clippy --release --workspace --locked --all-targets -- -D warnings` 通过。
4. PDFium manifest 与当前构建产物一致。
5. 启动 App 显示 LibraryWindow。
6. 从 LibraryWindow 打开 `tests/fixtures/acceptance.pdf`。
7. PDFWindow 显示，页面非空白，Preview 先出现，Tile 后续覆盖。
8. 鼠标/滚轮滚动至少 20 次，页面内容随视口正确更新，无白屏/崩溃。
9. 缩放至少 3 档，页面清晰度随 zoom 更新，常见比例不闪烁。
10. 关闭 PDFWindow 后 LibraryWindow 重新显示。
11. release self-test 通过。
12. GPUI fork/patch（如有）与 vendor check 一致。
13. 文档与代码状态一致。

完成以上即达成用户要求的“Rust 版本 LibraryWindowController → 打开 PDF → PDFWindowController → 渲染 → 滚动 → 缩放”。
