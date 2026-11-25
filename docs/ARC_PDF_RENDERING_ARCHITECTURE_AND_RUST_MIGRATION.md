# Arc PDF 渲染架构分析与 Rust + GPUI 重构规划

> 文档日期：2026-11-24
> 性质：源码分析 + 架构设计 + 工程规划。**不包含代码迁移、不修改现有源码、不修改 PDFium。**
>
> 标注约定（全文严格区分）：
>
> - **【事实】**：在指定路径/行号可直接读到的源码、配置、脚本、文档内容。
> - **【推论】**：基于源码事实的架构判断，尚未通过运行时 profiling 或实机复现。
> - **【建议】**：面向未来 Rust + GPUI 项目的设计选择，不是现有实现。
> - **【未确认】**：本次只读分析无法确定，需后续构建/测试/询问确认。
>
> 分析基线：
>
> | 仓库 | 本地 HEAD | 用途 |
> |---|---|---|
> | `/Users/danielhu/Projects/Arc-macOS` | `01949d0e` | 现有 macOS App、渲染视图层 |
> | `/Users/danielhu/Projects/ArcPDFKit` | `64d0650` | Swift PDFium 封装层 |
> | `/Users/danielhu/Projects/Arc-PDFium` | `4de7507` | PDFium 构建、XCFramework、C/C++ 扩展 |
> | `/Users/danielhu/Projects/Github/quiet-md` | `6102152` | Rust + GPUI 工程实践参考 |
>
> 一句话结论：**【推论】** 现有架构的核心价值不是 CATiledLayer，而是“低清整页底图 + 分页/分块按需高分辨率渲染 + 单一 PDFium 串行队列 + 缓存失效协议”；未来应在 Rust 中重建这套语义，而不是翻译 AppKit 类。GPUI 侧应自建页列表、自定义 `Element`、量化缩放桶、有界缓存与 PDFium actor，并把所有裸句柄和线程纪律封进 Rust 类型系统。

---

## 0. 阅读指引

本文按以下逻辑展开：

1. 第 1–3 章回答“现在是什么”：App 渲染链、Swift 封装、PDFium 构建与扩展。
2. 第 4 章回答“quiet-md 有哪些可迁移的工程做法”。
3. 第 5–6 章回答“未来 Rust + GPUI 应该怎样设计”。
4. 第 7 章给出分阶段实施路线与验收标准。
5. 第 8–9 章集中列出风险、未确认项与源码索引。

**重要边界**【事实/建议】：用户要求“Rust 采用相同的 preview + tile + 缓存策略”，因此本文保留该策略的**行为语义**（低清整页先显、高清分块覆盖、缓存复用、预取与失效），但明确不保留 `NSClipView`、`CATiledLayer`、`levelsOfDetailBias`、`NSView` 生命周期等 AppKit 机制。

---

## 1. 现有工程与依赖关系

### 1.1 依赖链与版本来源

**【事实】** App 通过 CocoaPods 消费 Swift 封装和 PDFium 集成：

- `Arc-macOS/Podfile:10-11`、`23-24`：`pod 'ArcPDFKit'` 与 `pod 'ArcPDFium'` 均指向 `zongmumask` 的 `main` 分支。
- `Arc-macOS/Podfile.lock:24-28`：依赖列表同时包含 `ArcPDFKit (from git ... branch main)` 与 `ArcPDFium (from ~/Projects/Arc-PDFium)`，即本地路径覆盖与 git 分支并存的混合状态。
- `Arc-macOS/Podfile.lock:50-52`：`ArcPDFKit` 的 checkout commit 为 `64d06507f2014dff92ddb8d6cbf7f6ef060fad77`，与本次分析基线一致。
- `ArcPDFKit/ArcPDFKit.podspec:28-29`：`s.dependency 'ArcPDFium'` 与 `s.dependency 'libwebp'`。
- `ArcPDFium/ArcPDFium.podspec:19`：`Extensions/**/*.{cpp,h}` 由消费者编译；`:28-31` vendored `Pdfium.xcframework` 与 `HarfBuzz.xcframework`。

**【推论】** 现有交付模型是“预构建二进制内核 + 消费者侧编译 C++ 扩展 + Swift wrapper”。这对 Swift 增量迭代友好，但把 ABI 风险沿内部 C++ 头传给了每个消费者，是 Rust 重构中最需要切断的耦合。

### 1.2 关键模块地图

**【事实】** 与渲染直接相关的文件：

| 层 | 路径 | 角色 |
|---|---|---|
| App 视图 | `Arc-macOS/Quill/PDFKit/View/PDFView.swift` | `NSScrollView`，持有 magnification、clipView、DocumentView |
| App 视图 | `Arc-macOS/Quill/PDFKit/View/PDFClipView.swift` | `NSClipView`，bounds 变化时通知代理 |
| App 布局 | `Arc-macOS/Quill/PDFKit/View/DocumentView.swift` | 页帧布局、可见页集合、预取边距 |
| App 页面 | `Arc-macOS/Quill/PDFKit/View/PageView.swift` | preview layer、tiled layer、失效管线、绘制回调 |
| App 瓦片层 | `Arc-macOS/Quill/PDFKit/View/PDFTiledLayer.swift` | `CATiledLayer` 配置 |
| App 常量 | `Arc-macOS/Quill/Utility/Constants.swift:97-113` | `levelsOfDetail = 32`、`previewScale(for:)` |
| App 缓存接线 | `Arc-macOS/Quill/Extensions/Document+.swift:38-49` | 用关联对象给 `Document` 挂 preview cache |
| App 缩略图 | `Arc-macOS/Quill/Preview/PageThumbnailProvider.swift` | 统一 2× base 缩略图渲染入口 |
| Swift 封装 | `ArcPDFKit/Classes/PDFium/Page.swift:244-362` | `renderRect`，Swift → PDFium 渲染主入口 |
| Swift 封装 | `ArcPDFKit/Classes/PDFium/Document.swift:139-151,258-280` | 全局串行 `pdfiumQueue`、per-document `decompressQueue` |
| Swift 缓存 | `ArcPDFKit/Classes/Render/FPDFRenderCache.swift` | preview / thumbnail / tile 三种缓存模式 |
| Swift 批注 | `ArcPDFKit/Classes/PDFium/AnnotationRenderer.swift` | 批注/widget 位图渲染 |
| C 集成 | `Arc-PDFium/scripts/build_pdfium.sh` | PDFium 拉源、构建、打包、导头、校验 |
| C 扩展 | `Arc-PDFium/Extensions/*.cpp`（22 个 `.cpp`） | 基于 PDFium 内部类的 C API 扩展 |

**【事实】** App 自身没有直接调用 `FPDF_RenderPageBitmapWithMatrix`；主页面渲染全部经 `ArcPDFKit.Page.renderRect`。App 中出现的 `FPDF_*` 多为批注创建/表单字段常量等非渲染路径（例如 `Quill/Interactions/ShapeDrawingSession.swift:18,407-460`、`Quill/PrepareForm/FormSelectionTool.swift:234-235`）。

---

## 2. 当前渲染实现分析（事实优先）

### 2.1 完整调用链

#### 2.1.1 滚动/可见页变化 → 屏幕

**【事实】** 链路如下：

```text
NSScrollView clip bounds 变化
  → PDFClipView.clipViewBoundsDidChange            Quill/PDFKit/View/PDFClipView.swift:56-64
  → FPDFClipViewDelegate.clipViewDidChangeBounds   PDFClipView.swift:11-13
  → PDFView.clipViewDidChangeBounds                PDFView.swift:330-333
  → DocumentView.visibleRectDidChange              DocumentView.swift:240-250
  → didScrollSinglePageScroll / didScrollDoublePageScroll
                                                  DocumentView.swift:252-302 / :331-385
  → makePageView(for:) → PageView.init             DocumentView.swift:506-511 / PageView.swift:107-116
  → PageView.setupViews                            PageView.swift:334-354
      ├─ setupPreviewLayer                         PageView.swift:203-208
      └─ setupTiledLayer                           PageView.swift:323-332
  → PageView.renderPreviewIfNeeded                 PageView.swift:210-278
  → Document.perform → Page.renderRect             ArcPDFKit Document.swift:258-269 / Page.swift:244-362
  → FPDFBitmap_CreateEx + FPDF_RenderPageBitmapWithMatrix
                                                  ArcPDFKit Page.swift:297-303 / :347
  → CGContext.makeImage → MainActor 写 previewLayer.contents
                                                  PageView.swift:264-276
```

**【事实】** `DocumentView.didScrollSinglePageScroll` 用 `pageViewFrames` 与 `visibleRect.insetBy(dx:0, dy:-preloadVerticalMargin)` 求交集，形成 `wantedVisibleSet`；新增页 `addSubview`，移除页走 0.15 秒延迟任务（`DocumentView.swift:255-301`、`:304-329`）。`preloadVerticalMargin = visibleRect.height * 0.8`（`DocumentView.swift:47-49`）。

#### 2.1.1.1 ExtendedVisibleRect 预加载策略（预览用扩展区，瓦片用严格可见区）

**【事实】** `didScrollSinglePageScroll` 的核心不是“只渲染可见页”，而是以一个纵向扩大的区域作为页面生命周期和预览的预加载带：

```swift
// Quill/PDFKit/View/DocumentView.swift:47-49
var preloadVerticalMargin: CGFloat {
    visibleRect.height * 0.8
}

// DocumentView.swift:255
let extendedVisibleRect = visibleRect.insetBy(dx: 0, dy: -preloadVerticalMargin)
```

`insetBy(dx: 0, dy: -margin)` 表示左右不变、上下各外扩 `visibleRect.height * 0.8`，即预加载带总高度约为 2.6 屏。具体语义：

| 用途 | 判定区域 | 源码位置 |
|---|---|---|
| 决定哪些页创建 `PageView`、进入 `visiblePageViews` | `pageViewFrame ∩ extendedVisibleRect` | `DocumentView.swift:255-270` |
| 决定哪些页触发 `renderPreviewIfNeeded()` | 对所有 `wantedVisibleSet` 调一次 | `DocumentView.swift:290-295` |
| 决定哪些页延迟移除 | 0.15 秒后再次检查 `pageFrame ∩ extendedVisibleRect`，相交则保留并取消移除 | `DocumentView.swift:304-329` |
| preview 的可见性门槛 | `isRectInPreloadRange` → `intersectsPreloadRange` 使用同一个 `extendedVisibleRect` | `PageView.swift:228-231,248-253,980-996` |
| tile 的可见性门槛 | `isRectVisible` / `intersectsVisibleRect` 使用**未扩展的** `documentView.visibleRect` | `PageView.swift:896,924,942,963-978` |

**【事实】** 这形成明确的两级策略：

- **Preview 用扩展区**：只要页/整页 rect 与 `extendedVisibleRect` 相交，就允许发起低清整页预览；滚动进入前已预热，减少白屏。
- **Tile 用严格可见区**：`drawTile` 的发起前检查、队列内 `shouldRenderBlock` 二次检查、渲染完成后 `setNeedsDisplay(rect)` 前的检查，均要求 tile 与 `documentView.visibleRect` 相交（`PageView.swift:896,924,942`）。预加载带中的页可以提前有 preview 和 `PageView`，但不会为屏外 tile 浪费 PDFium 串行队列时间。
- **延迟移除稳定化**：`removeInvisiblePages` 用 0.15 秒 debounce 并重查扩展区，避免快速来回滚动时反复创建/销毁 PageView；离开扩展区前若 `previewIsDirty`，会先 `renderPreviewIfDirty()` 预写缓存，滚回时命中（`DocumentView.swift:317-321`）。

**【推论】** 这是现有渲染架构中很值得保留的性能策略：预加载的是**轻量 preview 与视图容器**，高成本 tile 仍受严格可见区约束。未来 Rust 版本应显式表达为 `visible_set`（tile 用）与 `prefetch_set`（preview/PageState 用）两个集合，而不是把 0.8 屏魔法数字埋在 AppKit 布局代码里。

**【事实】** 双页模式 `didScrollDoublePageScroll` 使用同样的 `extendedVisibleRect` 与 `renderPreviewIfNeeded()` 流程（`DocumentView.swift:331-385`）。

#### 2.1.2 CATiledLayer 绘制 → 高清瓦片

**【事实】** 链路如下：

```text
AppKit 在渲染服务线程调用 CALayerDelegate.draw(_:in:)   PageView.swift:853
  → 从 ctx.ctm.a/d 取 targetScale（含 backing × magnification） :856-858
  → ctx.boundingBoxOfClipPath 切 128×128 逻辑格               :859-876
  → tileCache.cache(forKey:) 命中且 scale 相等 → ctx.draw      :883-889
  → 未命中/scale 不等 → drawTile(...)                         :902-907
  → document.perform { page.renderRect(...isRenderAnnot:false, renderForms:false) } :931-937
  → tileCache.setImage(...)                                   :934
  → 仅当真实产出新图且仍可见 → tiledLayer.setNeedsDisplay(rect) :942-943
  → 下一次 draw 命中缓存并 blit                                :883-886
```

**【事实】** `onPageRendered` 是一次性回调，preview 与瓦片两条路径都调 `invokePageRenderedHandlerIfNeeded()`（`PageView.swift:912,948-961`），用于“页面渲染完成”类导航/快照时序。

#### 2.1.3 内容变更 → 失效

**【事实】** 模型层写 PDFium 后经 `Document.invalidateDisplay` 闭包桥回视图层；`DocumentView` 注入的闭包在 `DocumentView.swift:87-92`：

```swift
document.displayInvalidator = { [weak self] pageIndex, pdfRect in
    guard let self, let pageView = self.visiblePageViews[pageIndex] else { return }
    pageView.reloadFormFields(in: pdfRect)
    pageView.invalidateContent(in: pdfRect)
}
```

`PageView.invalidateContentViewRect`（`PageView.swift:557-593`）做三件事：移除 preview cache 并置 `previewIsDirty`；收集与脏矩形相交的已缓存瓦片，在 `pdfiumQueue` 上按各瓦片原 scale 预渲回缓存；全部完成后一次 `tiledLayer.setNeedsDisplay(viewRect)`。

### 2.2 PageView 实现与生命周期

**【事实】**

- `PageView: NSView`，`index` 在 `init(page:)` 中从 `page.index` 拷贝一次并保存为 `let`（`PageView.swift:20-24,107-116`）。没有“复用时改 index”的路径。
- `setupViews()` 设置 `wantsLayer = true`、`layerContentsRedrawPolicy = .duringViewResize`，并依次加入 shadow layer、preview layer、tiled layer、文本高亮、批注画布与交互层（`PageView.swift:334-354`、`:203-208`、`:323-332`、`:415-460`）。
- `layout()` 每帧更新书签/阴影并调用 `renderPreviewIfNeeded()`（`PageView.swift:187-193`）。
- 移除页没有复用池：`DocumentView.removeInvisiblePages` 延迟 0.15 秒后重新检查扩展可见区，再 `removeFromSuperview()` 并清空字典槽（`DocumentView.swift:304-329`）。`scheduleDelayedRemoval/performPendingRemovals`（`:487-504`）全仓无调用方，是死代码。
- `deinit`（`PageView.swift:122-185`）取消通知观察；若文档未关闭，先在主线程捕获 session 数据，再把 commit/save 提交到 `document.perform`，依靠串行 FIFO 保证早于 `Page.deinit` 的 `FPDF_ClosePage` 与 `Document.deinit` 的 `FPDF_CloseDocument`。
- `PageView` 自身只观察主题变化；主题切换触发 `invalidateWholePageContent()`（`PageView.swift:494-501`）。滚动/缩放对它而言是间接的：由 `DocumentView` 的可见集合和 `NSView` 布局驱动。

**【推论】** 这套生命周期把视图-模型-引擎三者绑在 AppKit 的 `NSView` deinit 上，顺序正确性依赖 FIFO 与弱引用，难以在 Rust 中直接复刻。未来应由显式 `DocumentId/PageId` 与 actor 消息替代。

### 2.3 Preview 渲染机制

**【事实】** `PageView.renderPreviewIfNeeded()`（`PageView.swift:210-278`）：

1. 若 `previewLayer.contents != nil` 且 `!previewIsDirty`，只触发 `tiledLayer.setNeedsDisplay()` 后返回（`:212-216`）。
2. cache key 只是 `"\(page.index)"`（`:218`）；scale 来自 `Constants.previewScale(for:)`（`Constants.swift:103-113`），默认 2× 基准并把长边压到 1600 px。
3. 先在 `document.decompress`（per-document 串行 WebP 解码队列）读取缓存（`:235`），避免主线程解码阻塞。
4. miss 时在 `document.perform` 内再查一次缓存，随后 `page.renderRect(..., isRenderAnnot:false, renderForms:false)`，写 preview cache（`:243-259`）。
5. 回 MainActor 写 `previewLayer.contents`、`snapshotCache`、清 dirty、刷新阴影/高亮、触发回调与瓦片重绘（`:264-276`）。

**【事实】** preview cache 由 `Document+.swift:38-49` 以关联对象挂到 `Document`：`FPDFRenderCache(mode: .preview(maxCapacity: 1000 * 1024 * 1024))`，生命周期随 Document。

**【事实】** 脏路径 `regeneratePagePreview()`（`PageView.swift:282-314`）参数与常规路径不同：`isRenderAnnot: true`，`renderForms` 取默认值 `true`（`:297`）；而常规 preview 与瓦片均为 `false`（`:254`、`:932`）。项目已有文档 `docs/performance/text-editing-performance-analysis.md:250-254,280-297` 把这项不一致标为未优化。

**【推论】** 两处风险：

- 预览与瓦片在含批注/表单的页面上可能出现视觉跳变；这是源码事实直接支持的推论。
- `regeneratePagePreview` 在 `await document.perform` 后没有显式 MainActor hop 就写 `previewLayer.contents`（`:308-311`），而常规路径特意注明 UI 更新须主线程（`:264-265`）；这是潜在线程亲和性问题，**【未确认】** 实际运行是否触发异常。

**【事实】** preview 被明确定位为“滚动时秒出的低清整页底图”，不是瓦片未就绪时的兜底帧；项目性能分析文档 `docs/performance/text-editing-performance-analysis.md:43,76-113` 记录了从“每键重渲”改为“离开可见边界/再次展示/进网格前重渲一次”的优化。

### 2.4 CATiledLayer 与 Tile 渲染

**【事实】** `PDFTiledLayer`（`PDFTiledLayer.swift:16-24`）：

```swift
levelsOfDetail = Constants.levelsOfDetail          // 32
levelsOfDetailBias = Constants.levelsOfDetail - 1  // 31
let scale = NSScreen.main?.backingScaleFactor ?? 2.0
tileSize = CGSize(width: 512 * scale, height: 512 * scale)
```

在 2× Retina 下系统瓦片是 1024×1024 设备像素；`PageView.fixedTileSize = 128` view point，因此一次 `draw(_:in:)` 回调内部再切成 4×4 = 16 个逻辑格（`PageView.swift:85,847-913`）。

**【事实】** 关键机制：

- 绘制回调是 `CALayerDelegate.draw(_ layer: CALayer, in ctx: CGContext)`，不是 `drawLayer(in:context:)`（`PageView.swift:846,853`）。
- `targetScale = CGSize(width: ctx.ctm.a, height: ctx.ctm.d)`（`:856-858`），包含 backing factor 与 magnification。
- 格键 `tileIdentifier(x:y:) = "\(index)-\(x)-\(y)"`（`:915-917`），**键里不含 scale**；scale 存在缓存项里，绘制时做精确 `cached.scale == targetScale` 比较（`:887`）。
- 命中缓存先 blit 垫一帧；scale 相同则跳过异步重渲（`:883-889`），避免“自碰脏”反馈环。
- `drawTile` 只在 `renderRect` 真实返回图像且该格仍可见时请求 `setNeedsDisplay`（`:919-946`）；代码注释（`:928-930`）记录了此前丢弃返回值导致永久自刷的 bug。
- 内容失效时按缓存项存储的 scale 预渲相交瓦片，再一次刷新（`PageView.swift:570-591`）。

**【推论】** 连续缩放时 CTM 每帧变化，精确 scale 比较会使旧桶瓦片不断被判为不匹配并重新渲染；是否造成可见抖动取决于 AppKit CTM 的量化行为，**【未确认】**，未经 profiling。

**【事实】** `isRectVisible` / `isRectInPreloadRange` 从渲染线程用 `DispatchQueue.main.sync`（`PageView.swift:971-996`）。项目已有 `docs/performance/pdfiumqueue-sync-audit.md:10-37` 记录过主线程 ↔ `pdfiumQueue` 双向等待导致的实际死锁。**【推论】** 这类同步等待在 Rust 设计中必须消除。

### 2.5 Preview 与高分辨率 Tile 的协作

**【事实】** 层级顺序是 shadow → previewLayer → tiledLayer → NSView 叠加层（`PageView.swift:341-352`）。preview 是整页低分辨率底图，瓦片层覆盖其上。`draw()` 命中缓存直接把瓦片 blit 到当前上下文；未命中时先把已有瓦片/底图留在屏幕上，再异步补渲。

**【事实】** `PageView.onPageRendered` 由两条路径共同触发：preview 完成（`:274`）与瓦片 `draw` 结束（`:912`），消费者如 `Quill/Views/DraggableBezierView.swift:159` 依赖该回调。

**【推论】** 这是一个有效的“渐进式细化”协议，但回调语义较弱：它只表示“某次绘制/预览完成”，不区分 preview 还是 tile、不携带 scale/generation，容易造成上层时序耦合。Rust 版本应改为带元数据的显式事件。

### 2.6 调度、线程与缓存

#### 2.6.1 队列

**【事实】** `ArcPDFKit/Classes/PDFium/Document.swift`：

- `:139-151`：进程唯一串行队列 `com.arc.pdfium.serial.global`，qos `.userInitiated`。注释明确原因是 PDFium 进程级无锁全局状态（如 `CFX_FontMapper`），跨队列并发曾导致堆损坏/SIGABRT。
- `:258-269`：`perform<T>` 用 `withCheckedContinuation` + `pdfiumQueue.async`，`dispatchPrecondition(.onQueue(...))` 只在 DEBUG 生效。
- `:273-280` + `:183-185`：per-document 串行 `decompressQueue`，专门把 WebP 解码移出主线程，可与 PDFium 渲染重叠。
- `AnnotationRenderer.swift:25-29`：同样有 `dispatchPrecondition(.onQueue(Document.pdfiumQueue))`，注释记录过 AP 解析与 `FPDF_ClosePage` 竞态导致的历史 SIGSEGV/PAC 崩溃。

**【事实】** App 渲染路径没有 `OperationQueue`。`DocumentView.renderPriority: Operation.QueuePriority` 被设为 `.veryHigh`（`PDFView.swift:169`、`DocumentView.swift:39,68`），但全仓无消费者；唯一的 `OperationQueue()` 在搜索（`Quill/Controllers/SearchViewController.swift:104`）。

#### 2.6.2 缓存实现

**【事实】** `ArcPDFKit/Classes/Render/FPDFRenderCache.swift`：

- `Mode`: `.preview(maxCapacity:)`、`.thumbnail(maxCapacity:)`、`.tile`（`:10-14`）。
- preview/thumbnail 存 WebP 压缩字节（`:52-62`），tile 存裸 `CGImage`（`:63-72`）。
- 只有 `.preview` 在 `currentTotalSize > maxCapacity` 时驱逐最老项（`:87-89`、`:107-116`）；`.thumbnail` 虽声明 1 GB 上限，实际不触发驱逐；`.tile` 无容量/驱逐逻辑，随 PageView 生命周期销毁。
- `currentTotalSize` 是普通 `Int`（`:37`），在 `setImage`/`removeImage`/`evictOldest` 中读写，类声明 `@unchecked Sendable`（`:9`）。底层 `ThreadSafeDictionary` 只保证单次字典操作原子。
- preview cache 在 App 侧由 `Document+.swift:44` 配置为 1 GB；thumbnail cache 在 `ArcPDFKit Document.swift:48` 也配置为 1 GB 但上限无效。

**【推论】** 高并发下容量统计可能漂移；长会话中 thumbnail/tile 可能无界增长。App 与 ArcPDFKit 均 **【事实】** 没有 `didReceiveMemoryWarning` 或统一内存压力清理（本次 grep 无匹配）。

#### 2.6.3 去重与过期结果

**【事实】**

- `Page.renderRect` 在入口和 `pdfiumQueue` 内各检查一次 `shouldRenderBlock`（`Page.swift:253-255,338-341`）。
- preview miss 路径在队列内二次查缓存（`PageView.swift:244-253`）。
- 瓦片 `shouldRenderBlock` 同时检查可见性与是否被其它任务填了同 scale 缓存（`PageView.swift:894-901`）。
- 串行队列使后到的重复任务通常看到前一个已写缓存并返回 nil；这是事实上的去重，但没有显式 in-flight 注册表。`regeneratePagePreview` 的 `Task.isCancelled` 只在 `await` 之后检查（`PageView.swift:306`），不能取消已入队的渲染。

### 2.7 Zoom / Scroll / Viewport / 坐标转换

**【事实】**

- `PDFView` 是 `NSScrollView`（`PDFView.swift:13`），`allowsMagnification = true`，`minMagnification = 0.1`、`maxMagnification = CGFloat(Constants.levelsOfDetail)` = 32（`:162-165`、`Constants.swift:97`）。
- Fit width：`updateFitWidthMagnification()` 用 `(frame.width - edgeInsets.left - edgeInsets.right) / layoutView.contentSize.width`，变化大于 `0.0001` 才写入（`:229-237`）。`zoomToFitPage` 设 magnification = 1.0（`:318-321`）。
- `magnification.didSet` 调 `setZoomingWithDelay()` 并广播通知（`:191-196`）；`NSScrollView+.swift:32-40` 用 3 秒定时把 `isZooming` 置回 false。**【事实】** `isZooming` 全仓无读取方，因此没有“缩放中降质”策略落地。
- 页几何：`Document.pageSize(_:)` 返回 `FPDF_GetPageSizeByIndex` 结果并 `floor`（`ArcPDFKit Document.swift:692-720`）；`DocumentView.singlePageScrollViewSize/doublePageScrollViewSize` 生成 `pageViewFrames`（`DocumentView.swift:387-485`）。
- 局部渲染的坐标由 `Page.renderRect` 处理：`area = rect * scale`（`Page.swift:257-263`）；macOS 用 `pageSize.height - rect.origin.y - rect.size.height` 翻转 Y（`:313-321`）；矩阵为 `FS_MATRIX(a:scale.width, d:scale.height, e:-area.origin.x, f:matrixF)`（`:323-330`）。
- PDF → view 的 cropBox 换算统一为 `view = pdf - cropBoxOffset`（`PageView.swift:532-540`）；cropBoxOffset 在 `Page.loadIfNeeded` 中由 cropBox.origin - mediaBox.origin 得到（`Page.swift:177-199`）。项目文档 `docs/performance/image-editing-performance-analysis.md:59-95` 记录过历史上两种换算方向并存导致的错位 bug。
- `PDFClipView.contentOffset` 由 KVO 观察，供 `PDFViewController.swift:248-254` 更新页码指示器。

**【推论】** 当前坐标体系由三套量共同决定：PDF point、view point（受 magnification 影响）、device pixel（受 backing 影响），且分散在 App (`NSView.convert`)、Kit (`FS_MATRIX`) 和 AppKit (`ctx.ctm`) 三处。Rust 版本应把它们收敛为显式的 `PagePoint / PagePixel / ViewPoint / DevicePixel` 类型与一次集中转换。

### 2.8 PDFium 渲染接口与封装

**【事实】** `ArcPDFKit/Classes/PDFium/Page.swift:244-362` 是主渲染入口：

```swift
let pdfBitmap = FPDFBitmap_CreateEx(Int32(width), Int32(height),
    Int32(FPDFBitmap_BGRx), bitmapData, Int32(bytesPerRow))   // :297-303
defer { FPDFBitmap_Destroy(pdfBitmap) }                        // :306
var options = FPDF_REVERSE_BYTE_ORDER | FPDF_NO_CATCH          // :308
if isRenderAnnot { options |= FPDF_ANNOT }                     // :309-311
loadIfNeeded()                                                 // :345
FPDF_RenderPageBitmapWithMatrix(pdfBitmap, handle, &matrix, &clip, Int32(options)) // :347
if renderForms, let formHandle = document.formHandler?.handle {
    FPDF_FFLDrawWithMatrix(formHandle, pdfBitmap, handle, &matrix, &clip, Int32(options)) // :350
}
if themeMode == .night { applyNightModeToBitmap(pdfBitmap) }     // :353-355
outputImage = context.makeImage()                               // :357
```

**【事实】** 其他要点：

- CGContext 是 sRGB、`noneSkipLast`，背景填白（`:265-287`）；PDFium 直接写入 CGContext 的内存，`FPDFBitmap_Destroy` 只销毁 bitmap 对象，不释放这段 buffer。
- `FPDF_NO_CATCH` 在 vendored 头 `Arc-PDFium/Vendors/Pdfium.xcframework/macos-arm64_x86_64/Headers/fpdfview.h:833-837` 明确标为 obsolete/no-op。
- `FPDF_FFLDrawWithMatrix` 是 Arc 扩展，不是 PDFium public API；实现见 `Arc-PDFium/Extensions/fpdf_formfill_ext.cpp:68-118`，内部用 `CFX_RenderDevice::CreateForBitmap` 与 `CPDFSDK_PageView::PageView_OnDraw`。`docs/pdfium_upgrade_migration.md:37-54` 说明 public `FPDF_FFLDraw` 只有 start_x/start_y，没有矩阵能力，因此该扩展仍必要。
- 批注/widget 渲染走 `FPDFAnnot_RenderToBitmap` 与 `FPDF_Widget_RenderToBitmap`（`AnnotationRenderer.swift:149,225`），使用独立的 `FPDFBitmap_Create` 与透明背景（`:113-119,209-220`），并由 `PageView` 的 `AnnotationCanvasView` 以 `2^ceil(log2(backing*magnification))` 的桶化 scale 单独渲染（`Quill/Views/AnnotationCanvasView.swift:408-414`）。

**【事实/推论】** 存在两个同名 `CGImage.fromPDFiumBitmap` 重载：`Classes/Infrastructure/RenderShims.swift:51-68` 直接以 PDFium buffer 建 CGContext（不拷贝），`Classes/Compatibility/CGImage+FPDF.swift` 的重载显式 `malloc`+`memcpy` 并由 `CGDataProvider` 持有。前者被 `AnnotationRenderer.swift:160` 使用，而 `defer FPDFBitmap_Destroy(bitmap)` 在 `:116` 已注册。**【推论】** 这依赖 `CGContext.makeImage()` 的内部拷贝语义，是值得在 Rust 版本中用显式 buffer 所有权消除的风险。

### 2.9 Bitmap / Page / Document 生命周期

**【事实】**

- `Page.loadIfNeeded()`（`ArcPDFKit Page.swift:163-208`）懒加载 `FPDF_LoadPage`，读取 rotation、mediaBox、cropBox、cropBoxOffset，并 eagerly 加载 TextPage。
- `Page.unloadIfNeeded()`（`:215-236`）用 `objc_sync_enter` + `unloaded` 标志保证幂等，闭包只捕获 `pageHandle/formHandle` 值不捕获 self（`:213-214` 注释说明捕获 self 会越过 deinit 造成悬垂），并在 `pdfiumQueue` 上调 `FORM_OnBeforeClosePage` + `FPDF_ClosePage`（`:230-235`）。`Page.deinit` 调它（`:238-240`）。
- `Document` 通过 `NSRecursiveLock` 保护 `pagesStorage/pageSizesStorage/pageCountStorage`（`Document.swift:189-210`），`page(at:)` 持锁创建/注册以避免重复句柄（`:722-730`）。Page registry 强持有所有 Page，注释说明是为了防止 Annotation 的弱 page 引用失效导致 undo 崩溃（`:202-205`）。
- `Document.deinit`（`:543-596`）设置 `isWillClose`，卸载页，捕获句柄后用 `pdfiumQueue.sync` 按顺序 `FORM_DoDocument...` → `FPDFDOC_ExitFormFillEnvironment` → `FPDF_CloseDocument` → 释放 formFillInfo。
- **【事实】** 全仓没有 `FPDF_DestroyLibrary()`；`configPDFLibrary()` 在每次 Document init 调 `FPDF_InitLibraryWithConfig`（`:600-605`），`KernelDescriptor.swift:19` 还单独调 `FPDF_InitLibrary()`。

**【推论】** 生命周期正确性依赖 Swift ARC + deinit + ObjC 同步原语 + 串行 FIFO 四者共同成立；任何一环被异步取消/重入打破都会回到历史上的SIGSEGV类问题。Rust 应把这些不变量编码为类型与 actor 消息。

### 2.10 当前架构的优点、瓶颈与问题清单

**【事实 + 推论】**

| 项 | 判断 | 依据/说明 |
|---|---|---|
| 低清 preview 先显，避免滚动白屏 | 优点 | `PageView.swift:203-208,210-278`；`docs/performance/text-editing-performance-analysis.md:43` |
| 单一进程级 PDFium 串行队列 | 优点（必要约束） | `Document.swift:139-151` 记录过并发导致堆损坏 |
| 脏矩形失效 + 缓存瓦片预渲 + 一次刷新 | 优点 | `PageView.swift:557-593`，避免逐块消失 |
| 缩略图统一渲染参数与 key 规则 | 优点 | `PageThumbnailProvider.swift:6-14,28-49`，解决历史 key 污染 |
| `--verify` 升级自检 | 优点但能力不足 | 见 3.1/3.6 |
| `previewScale` 精确 scale 比较导致缩放期反复重渲 | 瓶颈【推论】 | `PageView.swift:856-858,887` |
| `isZooming` 无读取方，无缩放期降质策略 | 缺口 | `NSScrollView+.swift:32-40`，全仓无 reader |
| `renderPriority` 无消费者，无真正优先级调度 | 缺口 | `PDFView.swift:169`、`DocumentView.swift:39` |
| `.thumbnail` 1 GB 上限无效、`.tile` 无驱逐、容量统计非原子 | 风险 | `FPDFRenderCache.swift:37,87-89` |
| 无内存压力处理、Page registry 强持有全部 Page | 风险 | `Document.swift:202-210`；grep 无 memory warning |
| `DispatchQueue.main.sync` 从渲染线程查可见性 | 风险 | `PageView.swift:971-996`；`docs/performance/pdfiumqueue-sync-audit.md` |
| Preview 重渲与瓦片参数不一致 | 风险 | `PageView.swift:254,297,932` |
| `regeneratePagePreview` 可能跨线程写 UI | 风险【推论】 | `PageView.swift:295-311` |
| 高 zoom 下 128pt × scale 的瓦片像素量按平方增长 | 风险【推论】 | `PageView.swift:85,905-906` |
| `预构建内核 + 消费者侧扩展` 的价值与内部 ABI 风险并存 | 设计取舍 | 价值见 3.4；B/C 层升级风险见 `docs/pdfium_upgrade_migration.md:1-59` |

---

## 3. ArcPDFium：PDFium 集成与构建体系（事实优先）

### 3.1 `scripts/build_pdfium.sh` 全流程

**【事实】** 脚本共 195 行，`set -euo pipefail`（`:2`），顶部注释（`:4-16`）声明 6 步并明确“升级内核跑本脚本，自动追 pdfium main 最新 HEAD”，“源与 .a 同源，头/.a 错位会 ABI SIGSEGV”。

| 阶段 | 行号 | 实际行为 |
|---|---|---|
| 变量 | `:20-27` | `PDFIUM_URL`、`.pdfium_build` 工作区、`IOS_DEPLOY="17.0"` |
| 1 拉源 | `:34-59` | 无 depot_tools 则 clone；`gclient config --unmanaged ... --custom-var checkout_configuration=minimal`；写入 `target_os = [ 'mac', 'ios' ]`；`gclient sync --shallow --no-history`；打印 HEAD |
| 0 工具 | `:63-73` | 优先 `buildtools/mac/gn`，其次 PATH；ninja 优先 `/opt/homebrew/bin/ninja` |
| 导头 | `:75-93` | 镜像 `core fpdfsdk constants public build` 的 `.h`，另拷 FreeType include/src/include 到 `PdfiumHeaders/` |
| 校验 | `:97-119` | 对每个 `Extensions/*.cpp` 跑 `clang++ -std=c++20 -c`，失败打印前 5 条 error 并退出 |
| 模式分支 | `:121-127` | `--headers` 与 `--verify` 行为相同：都 `export_headers` + `verify_ext` 后退出 |
| 2 macOS | `:130-140` | `gn gen` arm64/x64 两目录，`ninja -C ... pdfium`，`lipo -create` 成 fat `libpdfium.a` |
| 3 iOS | `:143-164` | 两处 Perl 就地补丁（`:145-153`）；`grep -c` 断言（`:154-155`）；device/simulator 两套 GN args + ninja |
| 4 打包 | `:169-184` | `xcodebuild -create-xcframework`，三个 slice 的 headers 都取 `PDFIUM_DIR/public` |
| 5 再导头 | `:186-188` | 再次 `export_headers` |
| 6 提示 | `:190-195` | 只 echo `git add/commit/push` 建议；脚本本身不提交、不推送、无清理 |

**【事实】** GN args 要点（`:132,134,161`）：`is_component_build=false is_debug=false pdf_enable_v8=false pdf_enable_xfa=false pdf_is_standalone=true use_custom_libcxx=false pdf_is_complete_lib=true pdf_use_partition_alloc=false symbol_level=0 treat_warnings_as_errors=false clang_use_chrome_plugins=false`；mac 另设 `target_os="mac"`，iOS 另设 `ios_deployment_target="17.0" ios_enable_code_signing=false target_os="ios" target_environment=device|simulator use_blink=true` 等。

**【推论】** 这是一条面向单机、手工触发的发布流水线，不是可重复、可审计的构建系统。对 Rust 项目而言，**GN args 组合、slice 矩阵、`ninja pdfium → obj/libpdfium.a`、iOS 的 `fx_apple_impl` 补丁知识可复用；Perl 正则补丁、追 main、隐藏工作区不应复用**。

### 3.2 源码版本与可重现性

**【事实】**

- 脚本没有 revision pin；`gclient config --unmanaged` 后 `gclient sync` 跟随默认分支 HEAD（`:44-57`）。README.md:47-59 与脚注 `:14` 明确“自动追最新”。
- 本机 `.pdfium_build/pdfium` HEAD 为 `bc5820734ee8f94b68fed1093d8e6d7028155814`（2026-10-11），`git describe` 为 `bc58207`。
- `.pdfium_build/` 与 `Vendors/pdfium/` 均由 `.gitignore:6-7` 排除。本机前者 3.6 GB、后者 3.0 GB，均非 tracked。
- `docs/pdfium_unify_runbook.md:31-45` 仍写固定 SHA `686814d...`、`fd0c6c9...`，并引用已删除的 `scripts/fetch_pdfium.sh` 等脚本；`docs/pdfium_upgrade_migration.md` 记录的是 `b73d6847 → bc58207`。
- ArcPDFium 仓库 **【事实】** 无 `.github/` tracked workflow（`git ls-files '.github/*'` 为 0），只有 tag `0.1.0`；`ArcPDFium.podspec:3` 版本仍是 0.1.0。

**【推论】** “版本真相”散落在提交消息、迁移文档、本机工作树与二进制里，缺少机器可读清单，无法自动验证“头 == .a == xcframework slice 同 commit”。这与 runbook `:215` 自己列出的完成判据相矛盾。

### 3.3 构建产物与交付

**【事实】**

- `Vendors/Pdfium.xcframework`（约 70 MB）三个 slice：`macos-arm64_x86_64`（arm64+x64）、`ios-arm64`、`ios-arm64-simulator`，二进制统一名 `libpdfium.a`。
- `Vendors/HarfBuzz.xcframework`（约 46 MB）同样三 slice，二进制名 `libharfbuzz-combined.a`。仓库内 **【事实】** 没有重建 HarfBuzz 的脚本。
- 版本不一致已可观察：`harfbuzz` 产物 mac slice `hb-version.h` 为 `12.3.2`，iOS slice 为 `14.2.1`；当前 `.pdfium_build/pdfium/DEPS:165` pin 的是 `ff36703b95013ec1d437e6038e1aa7c881275f4e`。**【未确认】** 这些产物的确切构建源和参数。
- `ArcPDFium.podspec:15-16`：iOS 15.0 / macOS 13.0；脚本 `IOS_DEPLOY=17.0`（`:27`）与之不一致。
- `ArcPDFium.podspec:49-50` 的 header search path 仍指向被 gitignore 的 `Vendors/pdfium` 源树（ICU `unicode/uchar.h` 所需），而 `PdfiumHeaders/` 不含 ICU 头。**【推论】** 全新消费端若走需要该头的路径可能失败；本机因有隐藏源树未暴露。
- `ArcPDFium.modulemap` 显式列 39 个头（22 标准 + 17 扩展）；`Extensions/` 共 22 个 `.cpp`、45 个文件。未暴露的扩展头有 `cpdf_creator.h`、`cpdf_pagecontentgenerator.h`、`cpdf_security_handler.h`、`fpdf_import_common.h`、`fpdf_page_extract_ext.h`、`fpdf_security_ext.h`。

### 3.4 Core 扩展机制：分层的“预构建内核 + 消费者侧 C 扩展”模型

**【事实】** 现有设计不是“把 PDFium 源码全量翻译/全量重编”，而是有意拆成两部分：

1. **预构建内核**：`scripts/build_pdfium.sh:130-184` 只构建上游 PDFium 并打包 `Pdfium.xcframework`，产出 `libpdfium.a`；构建成本高，因此低频升级。
2. **消费者侧扩展**：`ArcPDFium.podspec:19` 把 `Extensions/**/*.{cpp,h}` 作为 `source_files`，由 pod 消费者编译；因此扩展源码改动是秒级增量，不需要重编内核。

**【事实】** 扩展对外几乎都是 `extern "C"` C API，风格刻意贴近 PDFium public API：使用 `FPDF_DOCUMENT/FPDF_PAGE/FPDF_BITMAP` 等不透明句柄、`FPDF_EXPORT`/`FPDF_CALLCONV`、纯 C 头。例如 `Extensions/fpdf_formfill_ext.h:10-32` 声明 `FPDF_FFLDrawWithMatrix` 与 `FPDF_Widget_RenderToBitmap`，`Extensions/fpdf_page_ext.h:16-25` 声明 `FPDFPage_IsContentStreamsDirty`。这让 Swift 侧调用方式与标准 `FPDF_*` 一致，也是这套方案的核心价值。

**【事实】** 但“扩展对外的 C 形态稳定”不等于“扩展内部实现都不依赖内核版本”。按实现依赖可分层：

| 层级 | 扩展类型 | 源码事实 | 稳定域 |
|---|---|---|---|
| A | public-only C 扩展 | `fpdf_imageio_ext.cpp` 只 include `fpdf_imageio_ext.h`、`fpdf_edit.h` 等 public 头，调用标准 `FPDF_*` API | 接近 public API 稳定性；不触碰内部 C++ ABI |
| B | 通过 FPDF 句柄访问内部对象模型的 C 扩展 | 如 `fpdf_bookmark_ext.cpp:3-13` include `core/fpdfapi/parser/cpdf_dictionary.h` 等内部头，用 `CPDFDictionaryFromFPDFBookmark` 等 PDFium 内部 helper；`fpdf_page_ext.cpp`、`fpdf_pagelabel_ext.cpp`、`fpdf_text_ext.cpp` 等也属此类 | 对外 C 签名稳定，但内部 helper/类布局随 PDFium 提交变化 |
| C | 明确依赖内部行为的定制/补丁 | `fpdf_formfill_ext.cpp:8-26,68-118` 使用 `CFX_RenderDevice::CreateForBitmap`、`CPDFSDK_PageView::PageView_OnDraw`；`cpdf_creator_security.cpp:32` 用 `#define private public` 访问私有成员；`cpdf_pagecontentgenerator.cpp` 是上游文件的修改副本 | 与具体 PDFium 提交强绑定 |

**【事实】** 这种分层的直接证据是 `docs/pdfium_upgrade_migration.md:9-35`：`b73d6847 → bc58207` 升级时，`CFX_DefaultRenderDevice` 被删除改为 `CFX_RenderDevice::CreateForBitmap`、`CountChars→CharCount`、`GetCharWidthF→GetCharWidth`、内部头迁移、alignment 强类型化、FreeType include 顺序变化。这些不是 public C API 漂移，而是 B/C 层依赖的内部 C++ 漂移。

**【推论】** 因此，“扩展与内核 ABI 强绑定”这个说法需要收窄：

- 对 **A 层**，风险很低，保留价值很高：public C 契约稳定、扩展迭代快。
- 对 **B 层**，对外 C 契约稳定，但实现依赖 PDFium 内部 helper，升级时需要适配；风险可通过同源清单与升级 CI 管理。
- 对 **C 层**，确实与具体内核提交强绑定；应集中标注、单独 pin、重点回归。

**【建议】** 未来 Rust 项目不应把“预构建内核 + 消费者侧扩展”整体抛弃，而应把它升级为更明确的版本化扩展模型：对外保留稳定的 C ABI，对内按 A/B/C 分层管理；A 层尽量成为可独立编译的 C shim，B/C 层的源码与 PDFium SHA 绑定，并在升级时自动验证。

### 3.4.1 扩展策略的备选方案比较【建议】

用户提出的设计动机是：public API 稳定但能力不足，扩展按 public 写法设计，预构建 framework 避免每次重编内核。**【推论】** 这是一个合理取舍；下面是基于该动机的方案比较，而不是否定现方案：

| 方案 | 做法 | 优点 | 代价 | 适用判断 |
|---|---|---|---|---|
| **预构建内核 + 消费者侧扩展**（现有方案） | 内核低频构建；扩展随消费者编译 | 扩展迭代快；public-style C API 好调用；不需要每次重编 PDFium | B/C 层升级时需适配内部头文件；需要同源与 ABI 验证 | **最适合保留其设计思想**，但要工程化治理 |
| 全量 fork PDFium，把扩展编入内核 | 修改/新增 PDFium 源文件，统一产出静态库 | 扩展与内核同一次构建，B/C 层 ABI 一致性最好 | 每次扩展改动都要重编内核；fork 升级冲突成本高 | 仅在大量 C 层扩展、且已维护稳定 fork 时合适 |
| 纯 public API 重写 | 完全不用内部头，只用 `FPDF_*` | 兼容性和升级成本最低 | 现有部分能力不可能或很难实现（如带矩阵的 FFL 绘制、字形级提取） | 可作为 A 层目标，不适合一刀切 |
| 独立页面渲染层（MuPDF/PDFium 双引擎抽象） | 扩展能力由上层引擎抽象，不直接依赖 PDFium 内核 | 可替换内核；跨引擎统一 | 工程量大；渲染一致性难保证 | 长期可选，不适合近期路线 |
| 动态加载扩展包/插件 | ABI 版本化 plugin，运行时加载 | 可独立发布扩展 | 跨平台打包、签名、沙盒、调试复杂；桌面端尤其麻烦 | 除非有第三方扩展生态，否则不建议 |

**【建议】** 近期最佳路线不是二选一，而是：

```text
保留“预构建内核 + 消费者侧扩展”的快速迭代优势
  + 给扩展包增加 PDFium SHA / ABI level / 导出符号 清单
  + A/B/C 分层
  + Rust 构建脚本负责逐层验证
```

这样既不牺牲高扩展性，也把升级风险从“运行时 SIGSEGV 后排查”前移为“构建/CI 明确失败”。

**【事实】** 现有脚本已经具备这个方向的雏形：`scripts/build_pdfium.sh:97-118` 会逐个编译 `Extensions/*.cpp`，内核升级后可提前暴露内部 API 漂移；不足是只做 `-c`、输出固定 `/tmp/*.o`、没有链接/符号检查，也没有机器可读的 ABI level/同源清单。

### 3.5 现有构建/维护流程的优缺点

| 项 | 判断 | 依据 |
|---|---|---|
| 扩展秒级增量编译 | 优点 | `podspec:19`，`README.md` 设计目标表 |
| 预构建 xcframework 离线可编 | 优点（对 CocoaPods） | `README.md:13-14` |
| 单一脚本封装复杂 GN/打包 | 优点 | `scripts/build_pdfium.sh:1-195` |
| 自动追 main | **缺点** | 脚本 `:14,44-57`；无 pin，无法二分 |
| `--verify` 名不副实 | 缺点 | `:99` 的 `-I` 指向源码树而非导出的 `PdfiumHeaders/`；只 `-c` 不链接；README:71 写 14 个，实际 22 个 `.cpp` |
| 平台锁定 macOS | 缺点 | `buildtools/mac/gn`、`/opt/homebrew/bin/ninja`、`lipo`、`xcodebuild -create-xcframework`、BSD grep/perl |
| 产物进 git | 缺点 | tracked 85 + 1621 个文件，约 116 MB，仅两个 xcframework |
| 无 CI、无 ABI 清单 | 缺点 | 无 `.github/`；无 revision manifest |
| 文档过时 | 缺点 | runbook 固定 SHA 与已删除脚本；脚本库名 `libpdfium.a` 而 runbook 仍写 `libpdfium_universal.a` |

### 3.6 Rust 构建体系的复用与重构策略【建议】

应保留为**输入规格**而非直接复制：

1. GN args 矩阵与 target/os/environment 组合。
2. `ninja pdfium → obj/libpdfium.a` 的目标约定。
3. iOS 需要把 `apple/fx_apple_impl` 编入的两处补丁**语义**。
4. `export_headers` 的目录白名单（`core/fpdfsdk/constants/public/build` + FreeType）作为最小头集参考。
5. `docs/pdfium_upgrade_migration.md` 与 `.claude/artifacts/arcpdfium-regress.html` 作为“行为契约/回归清单”。

应替换：

1. 用 `pdfium-revision.toml`（或 Cargo metadata）同时 pin PDFium SHA、HarfBuzz SHA、DEPS 子依赖版本。
2. 两处 iOS 补丁改为 checked-in `.patch`，`git apply --check` 失败即构建失败。
3. 每平台独立 GN out 目录；macOS 才 `lipo`，不再把 XCFramework 当源真相。
4. Windows/Linux 用 `target_os="win"/"linux"` + 相应 `target_cpu`，depot_tools/CIPD 按平台引导并校验 checksum。
5. `--verify` 升级为 compile + link + `nm` 符号检查，且必须针对导出的头文件集合。

---

## 4. quiet-md 工程实践借鉴

### 4.1 Workspace 与 Core 边界【事实】

- `rust-toolchain.toml:1-4`：`channel = "1.99.1"`、`components = ["clippy"]`、`profile = "minimal"`。
- `Cargo.toml:25-30`：`members = ["crates/core"]`，`exclude = ["vendor", "tools/vendor-sync"]`，`default-members = [".", "crates/core"]`。
- `crates/core/Cargo.toml:1-3` 明确“No GPUI, so it builds and tests on its own”；根目录 `src/main.rs:93` 再导出 `fdl_core` 模块，UI 用 `crate::model` 等路径。
- `Cargo.lock` v4 且无 git source；GPUI 来自 `vendor/zed/crates/gpui`（`Cargo.toml:37`），由 `vendor/zed/upstream.toml:1-5` pin Zed commit `e2f00a41ac6789f0ae52b229567ad5715266e400`，`vendor/PATCHES.md` 记录 44 个编号 patch，`tools/vendor-sync` 负责 check/save/sync/refresh。
- `[profile.dist]`（`Cargo.toml:107-129`）：`inherits=release`、`lto="fat"`、`codegen-units=1`、`strip="symbols"`、`debug="line-tables-only"`、`split-debuginfo="packed"`。

**【建议】** Rust PDF 项目应直接采用这种“无 UI 引擎 crate + GPUI 应用 crate + vendor 治理”的骨架，而不是把 PDFium、调度、缓存塞进 GPUI View。

### 4.2 Rust / GPUI 职责划分【事实】

- 入口 `src/main.rs:227-280`：`gpui_platform::application().with_assets(...)`，`app.run(...)`，`:156-165` 用 `cx.open_window(...)` 创建 `Entity<Workspace>` 根视图。
- `src/app.rs:346-487` 的 `Workspace` 持有 `SharedStore`、editor entity、ScrollHandle、`Task`、`Subscription`、`RefCell<SearchCache>`；`:546-591` 集中订阅窗口/主题/搜索/退出事件。
- `src/editor/view.rs:21-48,309` 定义 `EditorEvent` 并 `impl EventEmitter<EditorEvent> for Editor`；`src/app.rs:830-900` 用 `cx.subscribe_in` 把 `Changed` 转成保存。
- 编辑流统一为 `checkpoint → Core 方法 → Editor::changed()`（`docs/ARCHITECTURE.md:34-44`、`AGENTS.md:18`），`Core.generation` 推进后渲染缓存据此判陈旧（`:1507-1519`）。
- 渲染：`src/editor/render.rs:1753` 用 GPUI `list(...)` 虚拟列表；`sync_items()`（`:1162-1249`）用 key diff + `splice` 只重测变化行；`src/editor/text.rs:835-1116` 的自定义 `Element` 把 `request_layout/prepaint/paint` 与 hit_test 共用同一布局缓存。

**【建议】** PDF 页列表可直接套用“虚拟列表 + key diff + splice”；文本选区、链接热区、批注锚点应套用“一次布局既绘制又命中测试”的自定义 Element 模式。

### 4.3 缓存、异步与资源管理【事实】

- `src/editor/image.rs:184-190` 描述自定义图片缓存：GPUI 全局 asset cache 会一直保留解码图直到退出；本项目让离屏图 30 秒或超过 64 MiB LRU 淘汰，上传后 `RenderImage::hollow()` 释放 CPU 像素。
- `:202-230`：`DECODERS` 用 `async_lock::Semaphore`，并发数 clamp 到 2–4；`QUEUE` 按绘制顺序 FIFO；`run_next()` 在 slot 已释放或窗口最新帧不再绘制该图时跳过任务。
- `:239-246,423-473`：2 秒 sweep；保护所有 live window 正在显示的图；按 LRU + TTL + 64 MiB 预算淘汰；`:486-493` 用旧尺寸占位。
- `:279-286` 按显示尺寸量化 cap，避免 resize 每帧重解码；`:496-504` macOS 调 `malloc_zone_pressure_relief`；`src/big_alloc.rs:1-52` 用 mmap/munmap 处理 ≥1 MiB 分配。
- `src/board/render_vello.rs:215-243`：CPU 光栅帧写进 `RenderImage::new(vec![Frame::new(img)])`，用 `window.paint_image(...)` 显示，旧帧 `drop_image` 后把 buffer 取回复用（BGRA 与 GPUI atlas 匹配）。
- `docs/MEMORY.md:7-24,77-135`：照片库 Home 内存从 1160 MB 降到 207 MB，并列出了按显示尺寸解码、IDCT 缩放、hollow、mmap 等实验数据。

**【建议】** PDF tile 的缓存策略应直接沿用这套“量化 + 限并发 + 过期跳过 + 旧图占位 + LRU/TTL + 上传后释放 CPU 副本 + 大块 mmap”的组合，而不是沿用 `.tile` 裸 `CGImage` 无上限模型。

### 4.4 验证、CI、发布【事实】

- `scripts/check.sh` → `scripts/check.py:11-26`：`cargo test --release --workspace --locked`；ignored round-trip 测试；bundled asset 字节一致；`cargo clippy --release --workspace --locked --all-targets -- -D warnings`，并分别带 `devtools`、`devtools,datatable` features 再跑。
- `crates/core/src/editor/fuzz_tests.rs:1-4,678-686`：确定性随机编辑 fuzz，xorshift64*，`FDL_FUZZ_SEED=k` 重放、`FDL_FUZZ_SEEDS=n`、`FDL_FUZZ_STEPS=m`；每步检查不变量，含序列化往返与全量 undo/redo。
- `tools/harness/README.md:1-36,89-106`：`FDL_SCRIPT` 仅在 `devtools` feature 下编译进二进制；`compare.py` 逐像素比较新旧二进制截图；`src/selftest.rs:1-14` 是发版内置 `--self-test`，两阶段启动、临时库、状态/文件断言而非像素断言。
- `.github/workflows/ci.yml:23-122`：macOS-15 跑 build + check + arm64/x64 两个 ad-hoc 包 + Rosetta update 测试 + release self-test；`:124-203` Windows x64；`:205-263` Linux x64。Actions 用 commit SHA pin。
- `scripts/release.sh:1-105`：`--profile dist`、lipo 校验、codesign/notarize/staple、dSYM 单独保存、SHA-256；`docs/releases/0.1.8.md`–`0.1.13.md` 逐版本说明；`docs/UPDATES.md` 描述 minisign 清单与强制更新策略。
- `AGENTS.md:16-47`：**不要运行 rustfmt**；core 不依赖 GPUI；结构修改走 Core 并把新操作加入 fuzz；DocxXML fixture 字节级往返；移动型重构必须逐像素一致；保护真实用户 library，始终使用临时 `QUIET_MD_HOME`。

**【建议】** 未来 Rust PDF 项目应把这些流程当作模板：`check.py` 统一三平台检查、确定性 fuzz、像素回归、发布包内 self-test、SHA-pinned Actions、双 Cargo.toml 版本号同步、AGENTS/CLAUDE 同步约定。

### 4.5 明确不要照搬的部分【事实/建议】

quiet-md 的 DocxXML block 树、白板 Excalidraw 模型、多根目录 library、i18n 表、Mermaid WebView、minisign 更新策略都是业务/产品实现，**【建议】** 不要迁移到 PDF 应用。应迁移的是边界、generation、虚拟列表、缓存治理与验证流程。

---

## 5. Rust + GPUI 渲染架构设计【建议】

以下全部为未来设计，不是现有实现。

### 5.1 设计原则

1. **PDFium 只在一个进程级 actor 线程上运行**：对应现有 `Document.pdfiumQueue` 的不变量，但由 Rust 类型和消息边界强制。
2. **UI 永远不持有 PDFium 句柄**：GPUI 侧只持 `DocumentId/PageId/RenderKey`；句柄是 actor 私有字段。
3. **preview + tile 是策略，不是 AppKit 机制**：保留“低清整页先显 + 分块高清覆盖”，自己实现网格、scale 桶、缓存、取消。
4. **generation 是统一失效源**：任何内容/主题/表单状态变化推进 generation；缓存键与结果都携带它，过期结果直接丢弃。
5. **scale 量化**：连续 zoom 不进入缓存键；用离散桶（如 2 的幂或 1.25 步进），缩放中复用最近低桶，稳定后升档。
6. **有界资源**：CPU raster 和 GPU texture 都纳入字节预算；上传后释放 CPU 副本；大块 mmap。
7. **可验证**：渲染坐标、tile key、缓存/调度状态机都要能在无窗口 CI 中测试。

### 5.2 目录结构

```text
arc-pdf/
├─ Cargo.toml                       # workspace, profiles, lints, feature gates
├─ rust-toolchain.toml
├─ Cargo.lock
├─ crates/
│  ├─ arc-pdf-core/                 # 无 GPUI、无 PDFium：文档模型、几何、generation、命令
│  │  └─ src/{model,geometry,document,events,commands}.rs
│  ├─ arc-pdf-engine/               # PDFium actor：句柄、渲染请求、bitmap buffer
│  │  └─ src/{actor,handles,render,bitmap,error}.rs
│  ├─ arc-pdf-render/               # 无 GPUI 的调度与缓存策略：preview/tile/key/LRU
│  │  └─ src/{scheduler,keys,preview,tiles,cache,viewport,telemetry}.rs
│  ├─ arc-pdf-ui/                   # GPUI Workspace/PdfView/PageElement/overlays
│  │  └─ src/{app,pdf_view,page_element,overlays,input,theme}.rs
│  └─ arc-pdf-platform/             # 文件访问、系统 dialog、内存压力、平台适配 trait
│     └─ src/{fs,file_dialog,memory_pressure,mac,win,linux}.rs
├─ ffi/
│  ├─ arc-pdfium-sys/               # bindgen public C API；pin 的头文件
│  └─ arc-pdfium-ext/               # 少量必要 C ABI shim；显式标注内部 ABI 依赖
├─ pdfium/
│  ├─ pdfium-revision.toml          # PDFium/HarfBuzz SHA、DEPS 子依赖、patch 列表
│  ├─ patches/                      # 显式 patch 文件，不再 Perl 正则
│  └─ build.rs / scripts/build-pdfium.rs
├─ tests/fixtures/                  # PDF 样本 + golden 图 + manifest
├─ tools/{harness,perf,check.py,check.sh}
└─ docs/{ARCHITECTURE,RENDERING,PDFIUM,TESTING,BUILD,AGENTS}.md
```

依赖方向严格单向：

```text
arc-pdf-core  ← arc-pdf-engine  ← arc-pdf-render  ← arc-pdf-ui
       ↑                 ↑                 ↑
arc-pdf-platform（trait 定义；实现按 target gate 注入）
```

**【建议】** `arc-pdf-render` 不依赖 GPUI：它输出带元数据的 `Raster`。`arc-pdf-ui` 才把 `Raster` 转成 GPUI `RenderImage` 并 `paint_image`。这样渲染策略可以在无窗口 CI 中完整测试。

### 5.3 关键数据结构与接口示例

**【建议】** 以下接口示意用于说明边界，不是最终 API。

```rust
// arc-pdf-core：值与标识，无句柄
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DocumentId(u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PageId(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PageGeometry {
    pub width_pt: f32,
    pub height_pt: f32,
    pub crop_offset: Vec2,       // cropBox.origin - mediaBox.origin
    pub rotation: u8,
}

/// 所有会影响渲染输出的状态变化都推进它。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ContentGeneration(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RenderTheme { Day, Night }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderFlags {
    pub annotations: bool,
    pub forms: bool,
    pub theme: RenderTheme,
}
```

```rust
// arc-pdf-render：缓存键必须编码所有影响位图的维度
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScaleBucket(i16);      // 例如 2^(n/4)，连续 zoom 先量化

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TileRole { Preview, Tile, Thumbnail, Interaction }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileCoord { pub col: u32, pub row: u32 } // PDF point 空间 128pt 网格

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderKey {
    pub doc: DocumentId,
    pub page: PageId,
    pub role: TileRole,
    pub coord: Option<TileCoord>, // Preview 为 None（整页）
    pub bucket: ScaleBucket,
    pub generation: ContentGeneration,
    pub flags: RenderFlags,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestToken(u64);

pub struct RenderRequest {
    pub key: RenderKey,
    pub rect_pdf: RectF,          // PDF point，tile 为 128pt 网格交集
    pub pixel_size: SizeU32,      // 已按 bucket 计算，引擎不再猜 scale
    pub priority: Priority,       // VisibleTile > Preview > Prefetch > Thumbnail
    pub token: RequestToken,
}
```

```rust
// arc-pdf-engine：actor 私有句柄，不暴露到 UI
pub struct PdfiumDocumentActor {
    doc_handle: FpdfDocumentHandle,   // 私有 opaque handle 新类型，!Send 由 actor 线程约束
    pages: HashMap<PageId, PageHandle>,
    // form/annotation handles ...
}

pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub format: PixelFormat,      // Bgra8Opaque / Bgra8Premul，显式
    pub bytes: Bytes,             // Arc<[u8]> 或 mmap-backed，显式所有权
}

pub struct RenderResult {
    pub key: RenderKey,
    pub token: RequestToken,
    pub raster: Option<Raster>,
    pub elapsed: Duration,
}

// UI 侧只通过 id 通信
pub enum EngineCommand {
    Open { doc: DocumentId, source: OpenSource },
    Render(RenderRequest),
    Cancel(RequestToken),
    Invalidate { doc: DocumentId, generation: ContentGeneration, dirty_pdf: Option<RectF> },
    Commit { doc: DocumentId, ... },
    Close { doc: DocumentId },
}
pub enum EngineEvent {
    Rendered(RenderResult),
    Geometry { doc: DocumentId, geometries: Vec<(PageId, PageGeometry)> },
    Invalidated { doc: DocumentId, generation: ContentGeneration },
    Error { doc: Option<DocumentId>, error: EngineError },
}
```

**【建议】** 关键取舍：`RenderKey` 包含 `generation` 与 `flags`，所以“编辑后旧预览”“夜间模式切换后旧瓦片”“含批注/不含批注的预览”不会互相污染。`RequestToken` 用于取消；结果回 UI 时再按 generation/token 校验一次。

### 5.4 PDFium 绑定与 actor

**【建议】**

1. `arc-pdfium-sys` 只 bindgen PDFium **public** 头：`fpdfview.h`、`fpdf_annot.h`、`fpdf_formfill.h`、`fpdf_text.h` 等，版本由 `pdfium-revision.toml` 固定。C 类型只出现在 sys crate 内部。
2. `arc-pdf-engine` 启动一个**进程级专用 OS 线程**，持有全部 PDFium 全局状态与 document/page/form handles。UI 通过 channel 发送 `EngineCommand`；引擎通过 channel/GPUI foreground hook 回传 `EngineEvent`。
3. 不开多个 PDFium 线程，也不把 PDFium 工作丢进 GPUI 的通用 `background_executor`；后者只用于非 PDFium 的解码/压缩/IO。
4. `PdfiumRuntime` 用 `OnceLock`-式守卫集中 `FPDF_InitLibraryWithConfig`；进程退出时显式 shutdown/drain，替代现有“每 Document init 一次、无 DestroyLibrary”的状态。
5. 句柄生命周期由 actor 消息管理：`Close { doc }` 按 FIFO 排在所有 `Render` 之后；不存在 Swift `deinit` 式隐式顺序假设。
6. 扩展能力按 3.4 的 A/B/C 分层审计，**保留“预构建内核 + 消费者侧 C 扩展”的快速迭代模型**：
   - **A 层 public-only**：保留为稳定 C ABI shim，优先用 public API 组合实现。
   - **B 层内部 helper**：对外 C 签名保持稳定，但与 PDFium SHA 绑定；CI 负责逐扩展编译/链接验证。
   - **C 层内部 ABI/私有成员**：如 `FPDF_FFLDrawWithMatrix`、字形级提取、加密 owner password、FreeText 度量等；集中到 `arc-pdfium-ext`，crate 版本/配置携带 PDFium SHA，patch 必须 checked-in。
   - **谨慎处理**：`#define private public` 与整份复制的 `cpdf_pagecontentgenerator.cpp` 不应原样翻译；应评估能否改为显式 fork patch、上游 patch 或 public API 重写。
7. `arc-pdfium-ext` 应暴露版本化 C ABI：`abi_level`、`pdfium_sha`、导出符号清单；Rust sys crate 在构建期校验，避免运行时才发现错配。
8. 编译/链接检查必须包含 `nm` 符号检查，防止扩展与内核重复定义同名 `CPDF_*` 符号。

### 5.5 Preview + Tile 调度与协作

**【建议】** 状态机放在 `arc-pdf-render`，与 GPUI 无关：

```text
viewport 变化
  → ViewportState::compute(doc geometry, scroll offset, zoom, window size)
  → 得到 visible_pages + prefetch_pages（上下 0.8 屏，沿用现有预取量级）
  → 对每个页面：
      1) 保证 Preview 请求存在：整页 PDF rect，ScaleBucket(target).down(1~2)
      2) 对可见区域按 PDF point 128pt 网格生成 Tile 请求：ScaleBucket(target)
      3) 查 cache/in-flight：命中则跳过；同 key 已在途则合并 waiter
      4) 按 Priority 入 actor 队列：VisibleTile > Preview > Prefetch > Thumbnail
  → 结果回 UI：
      - token 失效 / generation 不符 → 丢弃
      - 合格 → 写 cache → 通知对应页面 entity
  → 绘制：
      - 先画 preview（放大到当前 viewport，允许模糊）
      - 再画每个已就绪 tile：优先同 bucket；否则画更低 bucket 作占位
      - 缩放中不请求新 bucket；zoom settle（如 120–180ms 无变化）后批量升档
```

关键规则：

- **去重**：`cache` 与 `in_flight: HashMap<RenderKey, RequestToken>` 双查；重复请求只登记 waiter。
- **取消**：页面离开 prefetch 区、generation 推进、zoom bucket 改变时，给旧 token 发 `Cancel`；引擎在渲染前后各检查一次。
- **过期结果**：结果自带 `key.generation` 与 token，UI 侧再校验；不允许“渲染结果直接写 View 字段”。
- **脏矩形**：沿用现有“旧 bounds ∪ 新 bounds”语义；只重渲相交 tile；preview 置 dirty，在页面离开可见区/再次展示/进入缩略图网格时重渲一次（与 `docs/performance/text-editing-performance-analysis.md:100-113` 的优化一致）。
- **参数一致**：preview、tile、thumbnail 的 `RenderFlags` 由同一个策略函数生成，杜绝现有 `isRenderAnnot/renderForms` 不一致。
- **动画/瞬时缩放**：用最近低 bucket 的位图做 GPU 缩放，不进入 PDFium；这是替代 `levelsOfDetailBias` 的显式策略。

### 5.6 缓存与内存治理

**【建议】**

- 全局 `RenderCache`：key = `RenderKey`，value = `Raster` + `uploaded: bool` + `last_used` + `bytes`。
- 两级预算：
  - CPU raster 预算（例如 128–256 MiB，按设备内存分级）；
  - GPU texture 预算交给 GPUI atlas，但通过 `drop_image`/`hollow` 语义主动释放。
- 淘汰：LRU + TTL；优先淘汰非可见、低优先级 bucket、旧 generation；保护当前所有窗口正在显示的项（参考 `src/editor/image.rs:423-473`）。
- 上传后释放 CPU 副本；需要重绘时用 GPU 中已有纹理，不再持有 `Arc<[u8]>`。
- 大 raster 使用 mmap-backed buffer（参考 `src/big_alloc.rs:1-52`），释放即归还 OS。
- WebP 可选：preview/thumbnail 可保留 WebP 压缩以节省内存；tile 用裸 BGRA 避免频繁编解码。**【建议】** 编码/解码走 GPUI `background_executor` 或独立线程池，不占用 PDFium actor。
- 监听平台内存压力：`NSApplication.didReceiveMemoryWarningNotification`（macOS）、Windows/Linux 对应信号或自定阈值；触发后按预算强制 sweep。
- 统计：`cache_bytes`、`in_flight`、`stale_dropped`、`render_ms[role]`、`cache_hit_rate` 写入 telemetry，benchmark 可读取。

### 5.7 Zoom / Scroll / Viewport 与坐标

**【建议】** 用显式类型替代三套隐式坐标：

```rust
struct PagePoint { x: f32, y: f32 }     // PDF 用户空间，左下或右上由 PDF 决定，内部统一
struct PagePixel { x: u32, y: u32 }     // 已按 bucket 光栅化
struct ViewPoint { x: f32, y: f32 }     // GPUI 逻辑点，含 zoom
struct DevicePixel { x: u32, y: u32 }   // 含 window.scale_factor()
```

转换集中在 `ViewportState`：

- `PageGeometry.crop_offset` 是 PDF → view 的唯一平移来源；不再出现方向相反的 helper。
- macOS/iOS/Windows 的 Y 轴差异只在 PDFium 提交矩阵时处理一次；上层交互统一用 view 坐标。
- `bucket = quantize(zoom * scale_factor)`；`pixel_size = ceil(rect_pt * bucket)`，按 `ceil` 而非现有 `Int(...)` 截断（`Page.swift:270-271`）。
- 滚动偏移来自 GPUI 的 scroll handle / list state；页可见集合由 `page_frames ∩ (visible_rect + prefetch_margin)` 计算，不再依赖 `NSView.convert`。

### 5.8 GPUI View 设计

**【建议】**

- `Workspace`（仿 `quiet-md/src/app.rs:346-487`）是窗口根 `Entity`；持有 `DocumentId`、共享 store、engine channel sender、`Task`/`Subscription` 字段。
- `PdfView` 是 `Entity`，持有 GPUI `ScrollHandle`、页列表 state、`HashMap<PageId, Entity<PageState>>`；滚动事件调用 `cx.notify()` 并触发 `RenderScheduler::update_viewport`。
- 页列表用 GPUI `list(...)` 虚拟列表（参照 `src/editor/render.rs:1753`），`sync_pages()` 用 key diff + `splice`；key 包含 page id、generation、theme、geometry。
- `PageElement` 实现 GPUI `Element` 的 `request_layout/prepaint/paint` 三阶段（参照 `src/editor/text.rs:835-1116`）：
  - `prepaint` 计算 preview/tile 的绘制矩形、clip、hitbox，写入共享 LayoutCache；
  - `paint` 只读 cache，调用 `window.paint_image(...)` 画 preview 与 tiles；
  - 文本选区、链接、批注 hit-test 复用同一份几何。
- `Raster → RenderImage` 的转换只在 `arc-pdf-ui` 发生；`window.drop_image` 后回收 buffer 到缓存 spare（参照 `src/board/render_vello.rs:215-243`）。
- 异步结果通过 `cx.update`/`cx.spawn` 回 foreground 更新 entity；**不在 paint 期间修改实体状态**，用 `cx.defer`/`on_next_frame`（参照 quiet-md 的 `cx.defer` 用法）。
- 不移植 `CATiledLayer`：没有 `levelsOfDetail`、`levelsOfDetailBias`、系统 tileSize；所有网格、桶、优先级由 `arc-pdf-render` 明确控制。
- 平台层用 trait：`FileSource`、`FileDialog`、`MemoryPressure`、`SystemFonts`；macOS 的 security-scoped bookmark、Windows 的 WebView/注册表、Linux 的 fontconfig 都留在 `arc-pdf-platform`。

### 5.9 多页文档的性能与稳定性

**【建议】**

- 只为 visible + prefetch 页创建 `PageState`；离开 prefetch 区时取消任务、释放该页 tile cache，但保留 preview（若仍在全局 preview cache 预算内）。
- 文档结构变化（插入/删除/旋转/重排）推进 `doc_generation` 并使所有 page keys 失效；禁止沿用旧 page index 作为跨结构变化的稳定键。
- 页几何一次性批量读取，避免每页 `FPDF_GetPageSizeByIndex` 反复调用。
- 夜间模式/主题切换作为 `RenderFlags` 变化推进 generation，不做“原地反色”式就地修改，避免与缓存位图耦合。
- 表单/批注默认独立 overlay 渲染；只有需要烘焙进位图的路径才打开 flags，并计入同一个 key。
- 全局 actor 队列提供优先级；同一文档内可见页 tile 优先于预取；缩略图/导出等后台任务最低优先级。

---

## 6. Rust 工程体系规划【建议】

### 6.1 Toolchain、依赖与 feature

- `rust-toolchain.toml` pin 具体版本 + clippy；`Cargo.lock --locked` 参与所有 CI/发布命令。
- Feature 分离：`devtools`（harness 脚本驱动，仅开发构建）、`pdfium-debug`（渲染统计）、`internal-abi`（显式启用内部 ABI 扩展，默认关闭）。发布构建不含 harness 与调试注入，参考 quiet-md `devtools` 设计。
- Workspace lints 复用 quiet-md 模式：对 GPUI 闭包风格冲突的 clippy lint 显式 allow，但 CI 一律 `-D warnings`。
- GPUI 若需 fork/patch，建立 `vendor/zed/upstream.toml` + 编号 patch + `vendor_sync check` 流程；**【建议】** 不在 App 仓库里散落临时修改。

### 6.2 PDFium 构建与版本管理

1. `pdfium/pdfium-revision.toml` 示例字段：

```toml
[pdfium]
url = "https://pdfium.googlesource.com/pdfium.git"
revision = "<40-hex SHA>"

[harfbuzz]
revision = "<40-hex SHA>"

[patches]
files = ["0001-ios-apple-fx-impl.patch", "0002-test-gni-cert.patch"]

[gn.common]
is_component_build = false
is_debug = false
pdf_enable_v8 = false
pdf_enable_xfa = false
pdf_is_standalone = true
pdf_is_complete_lib = true
use_custom_libcxx = false
pdf_use_partition_alloc = false
symbol_level = 0
```

2. 构建入口 `scripts/build-pdfium` 按 target 生成 out 目录，调用 `gn gen` + `ninja pdfium`；macOS 可额外 `lipo` 生成本地 fat lib；不再把 XCFramework 作为源真相。
3. 每目标的静态库、头文件哈希、patch 应用结果写入 `build-manifest.json`，CI 与发布包校验。
4. `arc-pdfium-sys` 的 bindgen 输入必须来自同一 revision 的 `public/`；头文件与静态库哈希不一致即构建失败。
5. Windows/Linux 首次落地时先跑“只构建 + `FPDF_InitLibrary` + 渲染一个 fixture 页”的 smoke，再接入 UI。

### 6.3 本地脚本与 CI

**【建议】** 复制 quiet-md 的“一个检查入口”模式：

```text
scripts/check.sh         → exec python3 scripts/check.py
scripts/check.py         → cargo test --release --workspace --locked
                           cargo clippy --release --workspace --locked --all-targets -- -D warnings
                           # feature 矩阵：devtools / internal-abi
                           # PDFium manifest 一致性检查
                           # golden 渲染回归检查
scripts/render-bench.sh  → 渲染基准
scripts/pdfium-build.sh  → 多平台内核构建
```

CI 矩阵：

| Job | 平台 | 必做 |
|---|---|---|
| `check` | macOS arm64 | build、test、clippy、feature 矩阵、golden render、PDFium manifest 校验 |
| `pdfium` | macOS + Linux + Windows | 逐 target `gn gen + ninja pdfium`、头/库哈希、patch `git apply --check`、link/nm 检查 |
| `windows` | windows-2022 | cargo build/test/clippy + 包构建 + self-test |
| `linux` | ubuntu-22.04 | 系统依赖 + cargo build/test/clippy + 包构建 |
| `release` | macOS | dist profile、签名/公证、dSYM、SHA-256、发布包 self-test |

Actions 全部用 commit SHA pin；docs-only PR 可跳过昂贵构建（参照 `.github/workflows/ci.yml:8-12,23-122`）。

### 6.4 测试、fixture、fuzz、benchmark

**【建议】**

1. **单元测试**（`arc-pdf-core`/`arc-pdf-render`，无窗口）：
   - `cropBoxOffset` 方向、rotation、128pt 网格切分、`ceil` 像素计算、scale bucket 量化、cache key 稳定性。
2. **PDF 渲染 golden 回归**：
   - fixtures 覆盖：普通文本、嵌入字体、透明/alpha、cropBox ≠ mediaBox、旋转页、表单、批注、高 zoomtile、超大页、损坏页。
   - 输出 PNG 与 baseline 逐像素比较；允许明确记录的容差（抗锯齿差异）。
3. **确定性随机测试**（仿 `crates/core/src/editor/fuzz_tests.rs`）：
   - 随机 viewport 序列（scroll/zoom/pages structure change/theme toggle），断言：无重复 in-flight、cache bytes ≤ 预算、所有结果 generation 有效、过期 token 被丢弃、页离开后任务取消。
   - `FDL_PDF_FUZZ_SEED/SEEDS/STEPS` 重放。
4. **UI harness**：
   - `FDL_SCRIPT` 仅 devtools；脚本驱动滚动/缩放/页跳转/主题切换；`compare.py` 逐像素比较；`bench` 输出 frame/render ms。
5. **benchmark**：
   - 1000 页文档冷/热滚动、连续 zoom、编辑后脏矩形刷新、内存峰值；记录方法与环境，写入 `docs/PERFORMANCE.md`（参照 quiet-md `docs/PERFORMANCE.md:15-47`）。
6. **发布包 self-test**：
   - 内置 `--self-test`，临时目录、固定 fixture PDF、双阶段启动；断言渲染有内容、页导航、缩放、缓存统计、无崩溃；不依赖脚本驱动。

### 6.5 发布、版本与文档

- 两个 crate 与根包版本号同步；`CHANGELOG`/`docs/releases/<version>.md` 随发布提交。
- 发布包：macOS `.app`/zip + 签名公证；Windows installer；Linux tar/AppImage；dSYM/PDB 单独保存；SHA-256。
- 文档体系至少包含：
  - `docs/ARCHITECTURE.md`：模块边界、依赖方向、编辑/渲染流。
  - `docs/RENDERING.md`：preview/tile 状态机、坐标、缓存、失效。
  - `docs/PDFIUM.md`：revision、GN args、patch、扩展分类、ABI 约束。
  - `docs/TESTING.md`：fixture、golden、fuzz、harness、self-test。
  - `docs/BUILD.md`：本地构建、三平台、发布。
  - `AGENTS.md`/`CLAUDE.md`：同步约定、验证命令、禁止事项、用户数据保护规则。

---

## 7. Rust + GPUI 构建路线【建议】

### 阶段 0：基线冻结与契约定义

| 项 | 内容 |
|---|---|
| 目标 | 把现有实现与未来约束固化为可验证书面契约，不动业务代码 |
| 范围 | 四个仓库渲染/构建文档、fixture 清单、PDFium revision 提案 |
| 实施 | 整理本文事实清单；收集 10–20 个代表性 PDF；定义 `RenderKey/generation/flags` 语义；确定首个 PDFium SHA |
| 交付 | `docs/RENDERING.md` 初稿、`fixtures/manifest.json`、`pdfium-revision.toml` 提案 |
| 验证 | 每个事实条目能定位到文件路径；fixture 可被现有 Swift 路径渲染出 baseline |
| 风险 | 过早锁定 API；缓解：只锁语义不锁 Rust 签名 |
| 完成标准 | baseline PNG + 源码引用齐全， reviewers 确认无臆造 |

### 阶段 1：Workspace 骨架与纯 Core

| 项 | 内容 |
|---|---|
| 目标 | 建立无 GPUI、无 PDFium 的 core 与最小 GPUI 窗口 |
| 范围 | Cargo workspace、toolchain、check 脚本、CI 最小 job、`arc-pdf-core` 几何/文档模型 |
| 实施 | 定义 `PageGeometry`、`DocumentId/PageId`、`ContentGeneration`；写坐标/网格单元测试；GPUI hello window |
| 交付 | 可 `cargo test --workspace --locked` 通过的骨架；CI 首条绿线 |
| 验证 | `scripts/check.sh`；macOS/Windows/Linux 均能 build |
| 风险 | GPUI fork 版本不稳定；缓解：先固定 vendor rev 与 patch 流程 |
| 完成标准 | 三平台 CI 通过；core 测试无 GPUI 依赖 |

### 阶段 2：PDFium sys + Engine actor + 单页渲染

| 项 | 内容 |
|---|---|
| 目标 | 在 Rust 中复现 `Page.renderRect` 的最小可用路径 |
| 范围 | `arc-pdfium-sys`、专用 actor、`Raster`、单页 PNG 导出 |
| 实施 | bindgen public 头；`FPDF_InitLibrary` 单次初始化；`FPDF_LoadPage` + `FPDF_RenderPageBitmapWithMatrix`；显式 BGRA buffer；FIFO close |
| 交付 | CLI：`render --page N --bucket B --out page.png`；渲染 smoke 测试 |
| 验证 | 对阶段 0 baseline 做像素比较；DEBUG 断言 actor 线程唯一 |
| 风险 | 扩展能力缺失导致表单/批注差异；缓解：阶段 2 只比纯页面内容 |
| 完成标准 | fixture 中“无批注无表单”页面与 Swift baseline 一致（记录容差） |

### 阶段 3：Preview + Tile 调度与缓存（无 UI）

| 项 | 内容 |
|---|---|
| 目标 | 在 `arc-pdf-render` 中完整实现策略状态机 |
| 范围 | viewport、tile grid、scale bucket、preview、优先级、去重/取消/过期、有界缓存 |
| 实施 | `RenderScheduler`、`in_flight`、`RenderCache`、generation 失效、telemetry；确定性 fuzz |
| 交付 | 可跑的调度器测试与 benchmark；缓存命中/丢弃统计 |
| 验证 | 随机 viewport fuzz；cache bytes 不越界；无重复/过期结果 |
| 风险 | 量化桶选择影响清晰度；缓解：A/B 多组 bucket 并用 golden 图评估 |
| 完成标准 | fuzz 1000 seeds 通过；benchmark 输出稳定；策略文档与实现一致 |

### 阶段 4：GPUI 页视图与交互

| 项 | 内容 |
|---|---|
| 目标 | 在 GPUI 中显示 preview + tile，并支持滚动/缩放/页导航 |
| 范围 | `arc-pdf-ui`、`PageElement`、虚拟页列表、overlay 基础 |
| 实施 | `Raster→RenderImage` 桥；`request_layout/prepaint/paint`；key diff splice；文本/批注 hitbox |
| 验证 | harness 脚本：滚动、pinch、页跳转、主题切换；`compare.py` 像素回归 |
| 交付 | 可用的 PDF 阅读视图 |
| 风险 | GPUI API 与 quiet-md fork 有差异；缓解：先写最小 Element 试验，记录 vendor patch |
| 完成标准 | 基准 PDF 在三平台能滚动/缩放；无主线程阻塞；无过期画面 |

### 阶段 5：扩展能力审计与按需移植

| 项 | 内容 |
|---|---|
| 目标 | 处理“渲染正确但能力不完整”的扩展依赖 |
| 范围 | 22 个 `.cpp` 的 API 清单、调用方审计、分类迁移 |
| 实施 | public-only 重写优先；必须内部 ABI 的放入 pin SHA 的 `arc-pdfium-ext`；link/nm 检查 |
| 验证 | 表单/批注/字形提取/加密 fixture 对比；patch 在 CI 每 target 应用 |
| 风险 | ABI 漂移；缓解：crate 版本携带 PDFium SHA，升级必须重跑完整矩阵 |
| 完成标准 | 渲染相关扩展 100% 有明确归属；无未文档化的内部头依赖 |

### 阶段 6：内存与多页稳定性

| 项 | 内容 |
|---|---|
| 目标 | 长会话、大文档、高 zoom 下资源可控 |
| 范围 | GPU texture 回收、mmap、内存压力、预取淘汰、结构变化失效 |
| 实施 | 上传后释放 CPU 副本；LRU/TTL + 预算；大页 tile 分片上限；结构变化推进 doc generation |
| 验证 | 1000 页滚动 10 分钟、连续 zoom、编辑后刷新；内存/帧时报告 |
| 风险 | 过早淘汰导致白屏；缓解：低桶 preview 常驻 + 旧图占位 |
| 完成标准 | 内存峰值有明确上限；无泄漏增长；白屏率/重渲次数达标 |

### 阶段 7：跨平台构建、发布与自测

| 项 | 内容 |
|---|---|
| 目标 | 可重复构建、可发布、可回归 |
| 范围 | 三平台 CI、dist profile、packaging、self-test、release 文档 |
| 实施 | PDFium 每 target 构建；Roadmap 中逐步启用 Windows/Linux UI；发布包内置 `--self-test` |
| 验证 | CI 矩阵全绿；发布包在空格/CJK 路径下 self-test；签名/公证/SHA 校验 |
| 风险 | Windows/Linux 字体与系统依赖差异；缓解：平台 trait 隔离，CI 固定依赖版本 |
| 完成标准 | 一次 tag 触发完整构建→测试→打包→self-test；发布清单可复现 |

**路线原则**【建议】：阶段 0–3 不碰 GPUI 细节，先把“渲染语义 + 可测试性”做对；阶段 4 才接 UI；阶段 5 按能力而非文件迁移扩展；阶段 6–7 做稳定性与发布。避免一开始就复制 CATiledLayer 或全部 182 个扩展 C API。

---

## 8. 风险登记与总体验收

**【建议】** 按优先级：

| 优先级 | 风险 | 触发条件 | 缓解 |
|---|---|---|---|
| P0 | PDFium 全局状态并发 | 多 worker/多线程调用引擎 | 进程级单 actor；DEBUG 线程断言；测试并发压力 |
| P0 | 头/.a/SHA 不一致 | 升级内核只换一部分 | revision manifest + 哈希校验 + CI 失败即阻断 |
| P0 | 过期结果上屏 | 取消/失效路径遗漏 | key+generation+token 三重校验；fuzz 覆盖 |
| P1 | GPU texture / CPU raster 双份占用 | 缓存只增不减 | 上传后释放；LRU/TTL/预算；内存压力 sweep |
| P1 | 内部 ABI 扩展漂移 | PDFium 升级 | 分类迁移；pin SHA；patch CI；nm 检查 |
| P1 | 缩放抖动/重复渲染 | 精确 scale 作 key | 量化 bucket；缩放期复用低桶；settle 后升档 |
| P2 | GPUI fork patch 失控 | 直接改 vendor | vendor-sync 流程 + 编号 patch + 逐 patch 验证 |
| P2 | 测试样本不足 | 只测简单 PDF | fixture 矩阵与像素回归；损坏/边界样本 |

总体验收标准【建议】：

1. 事实层面：本文所有【事实】条目可在对应仓库 HEAD 复核。
2. 架构层面：`arc-pdf-core/engine/render` 可在无 GPUI 环境独立测试。
3. 行为层面：preview + tile + 缓存 + 失效语义在随机 viewport fuzz 下不变量成立。
4. 资源层面：CPU/GPU 缓存有明确上限，长会话无泄漏增长。
5. 构建层面：三平台 CI 可重复构建，PDFium revision/patch/产物可审计。
6. 发布层面：tag 触发完整流水线，发布包 self-test 通过。

---

## 9. 未确认事项

**【未确认】** 以下事项本次只读分析无法确定，不应作为既定结论使用：

1. 连续 zoom 时 AppKit CTM 的实际量化粒度，以及精确 scale 比较造成重渲的频率；需要 profiling。
2. `regeneratePagePreview` 在 `await document.perform` 后写 `previewLayer.contents` 是否在实践中总落在主线程；需要线程断言或日志验证。
3. `CGContext.makeImage()` 是否对 `RenderShims.swift:51-68` 零拷贝构造的 buffer 做了内部拷贝；需要运行时验证或改为显式拷贝。
4. 生产环境消费的 ArcPDFium 具体提交：`Arc-macOS/Podfile` 写 branch main，`Podfile.lock` 本地 path 覆盖；本地 ArcPDFKit pod checkout 的扩展头缺少 `FPDFExt_BorderStyle`、`FPDFExt_ShapeAppearance`、`FPDF_Widget_RenderToBitmap` 等声明，是否影响当前构建未确认。
5. `HarfBuzz.xcframework` 各 slice 的确切构建源/参数；mac 为 12.3.2、iOS 为 14.2.1，与当前 DEPS pin 的关系未确认。
6. `ArcPDFium` 扩展中 182 个 C API 的完整调用矩阵（哪些被真实使用、哪些可删除）需要结合 App/iOS/其他消费者审计。
7. Windows/Linux 上现有内部 ABI 扩展的可编译性与性能；当前脚本只覆盖 macOS/iOS。
8. 现有 App 的实际内存峰值、tile 命中率、滚动掉帧率；本文未运行 profiling。
9. quiet-md 中 `gpuimestone` fork 的说法在仓库中未发现；实际是 Zed 上游 `e2f00a41...` + 44 个 patch 的 vendoring。

---

## 10. 附录：关键源码索引

### 10.1 App 渲染

| 主题 | 路径与位置 |
|---|---|
| 滚动到可见页 | `Arc-macOS/Quill/PDFKit/View/PDFClipView.swift:56-64`；`PDFView.swift:330-333`；`DocumentView.swift:240-302` |
| 页创建/移除 | `DocumentView.swift:304-329,506-511` |
| Preview | `PageView.swift:210-278,282-321`；`Quill/Utility/Constants.swift:103-113`；`Quill/Extensions/Document+.swift:38-49` |
| Tile 配置 | `PDFTiledLayer.swift:16-24`；`PageView.swift:85,323-332,846-946` |
| 失效 | `DocumentView.swift:87-92`；`PageView.swift:516-593` |
| 可见性检查 | `PageView.swift:963-996` |
| Zoom | `PDFView.swift:162-165,191-196,229-236,308-333`；`Quill/Extensions/NSScrollView+.swift:32-40` |
| 已有性能分析 | `docs/performance/text-editing-performance-analysis.md`、`image-editing-performance-analysis.md`、`pdfiumqueue-sync-audit.md` |

### 10.2 ArcPDFKit

| 主题 | 路径与位置 |
|---|---|
| 主渲染 | `Classes/PDFium/Page.swift:244-362` |
| 队列 | `Classes/PDFium/Document.swift:139-151,183-185,258-280` |
| 生命周期 | `Page.swift:163-240`；`Document.swift:202-247,543-596` |
| 缓存 | `Classes/Render/FPDFRenderCache.swift:9-181` |
| 位图转换 | `Classes/Infrastructure/RenderShims.swift:17-68`；`Classes/Compatibility/CGImage+FPDF.swift` |
| 批注渲染 | `Classes/PDFium/AnnotationRenderer.swift:19-29,113-186,191-236` |
| Pod 边界 | `ArcPDFKit.podspec:15-39` |

### 10.3 ArcPDFium

| 主题 | 路径与位置 |
|---|---|
| 构建脚本 | `scripts/build_pdfium.sh:20-27,34-59,63-73,75-93,97-127,130-195` |
| 版本/忽略 | `.gitignore:6-7`；`README.md:47-59`；`docs/pdfium_unify_runbook.md:31-45`；`docs/pdfium_upgrade_migration.md:1-59` |
| 交付 | `ArcPDFium.podspec:15-103`；`ArcPDFium.modulemap:18-62` |
| 扩展 | `Extensions/fpdf_formfill_ext.cpp:8-26,68-118`；`Extensions/cpdf_creator_security.cpp`；`Extensions/cpdf_pagecontentgenerator.cpp` |
| 产物 | `Vendors/Pdfium.xcframework/Info.plist`；`Vendors/HarfBuzz.xcframework/Info.plist` |

### 10.4 quiet-md

| 主题 | 路径与位置 |
|---|---|
| Workspace/toolchain | `Cargo.toml:1-30,107-148`；`rust-toolchain.toml:1-4`；`crates/core/Cargo.toml:1-40` |
| GPUI vendoring | `vendor/zed/upstream.toml:1-43`；`vendor/PATCHES.md:1-44`；`tools/vendor-sync/` |
| App/Core 边界 | `docs/ARCHITECTURE.md:5-10,34-44,94-171`；`src/main.rs:93,156-165,227-280`；`src/app.rs:346-487,830-900` |
| 渲染/虚拟化 | `src/editor/render.rs:1162-1249,1507-1519,1753`；`src/editor/text.rs:835-1116` |
| 缓存/内存 | `src/editor/image.rs:184-190,202-246,279-286,423-504`；`src/big_alloc.rs:1-52`；`docs/MEMORY.md:7-24,77-135` |
| 验证/CI | `scripts/check.py:11-26`；`crates/core/src/editor/fuzz_tests.rs:1-4,678-686`；`tools/harness/README.md:1-106`；`src/selftest.rs:1-14`；`.github/workflows/ci.yml:23-263` |
| 规范 | `AGENTS.md:16-47`；`CONTRIBUTING.md:42-108` |

---

**文档结论**【建议】：未来架构应以“GPUI 只负责显示与交互、Rust Core 负责语义与策略、PDFium actor 负责唯一引擎线程、显式 pin 的构建体系负责可重复性”为四条主线。现有 Swift 实现提供的是行为规格和历史陷阱清单，而不是需要逐类翻译的代码结构。
