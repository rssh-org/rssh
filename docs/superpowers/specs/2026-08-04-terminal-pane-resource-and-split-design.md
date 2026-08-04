# 终端资源占用与同窗分栏设计规格

日期：2026-08-04
状态：设计已确定，待进入实现计划

## 1. 背景与问题

RSSH 当前在 macOS 上通过 Tauri `WebviewWindowBuilder` 为每个原生窗口创建独立 WebView。每个窗口还会加载一套 Svelte 应用、xterm 实例、事件监听和前端状态。用户反馈打开 3 个窗口约占用 550 MB。

当前“分屏”并不是单 WebView 内的 pane：`AppShell.openInNewWindow()` 调用 `open_tab_in_new_window`，Rust 端创建新 WebView，前端通过 `window.__rssh_clone` 克隆 tab。这样会重复应用运行时，并且把布局意图表现成“新窗口 + 新 tab”，交互不够清晰。

本次目标：

1. 为终端提供同一窗口内的真正分栏。
2. 分栏创建的终端会话在同一 WebView 内运行。
3. 保留现有“移到新窗口”作为明确的独立动作。
4. 不重写 SSH/PTY/AI/SFTP 后端边界。
5. 通过可复现的行为测试和 macOS 进程观察验证结果。

非目标：

- 本次不承诺把 3 个终端压到固定的 MB 数值。
- 不在没有 profile 数据的情况下重写 Rust 会话线程模型。
- 不把所有 tab 改造成跨窗口共享的前端 store。
- 不为移动端增加复杂的触摸分栏。

## 2. 方案决策

### 方案 A：同窗分栏

一个 Tauri WebView 内维护分栏树，每个叶子引用一个终端 tab。优点是消除分屏场景中额外的 WebView、AppShell 和应用级监听，交互最符合终端应用习惯。代价是需要引入布局树和 pane 渲染层。

### 方案 B：继续多窗口并优化创建

只优化窗口创建和克隆 payload。改动较小，但每个窗口仍有 WebView 和一套前端运行时，不能解决主要内存来源。

### 方案 C：同窗分栏并保留纯新窗口

默认使用方案 A，同时保留方案 B 的显式“在新窗口打开”入口。两条路径严格分离：分屏永远在当前 WebView 内完成；新窗口永远创建独立的 Tauri WebView，不承担分屏或平铺职责。

**决策：采用 C。** 用户的分屏动作进入同窗分栏；只有明确选择“在新窗口打开”时才创建原生窗口，且该动作不带 split direction、不移动或平铺当前窗口。

## 3. 布局模型

新增 `src/lib/terminal/layout.ts`，仅包含无副作用的布局数据结构与变换函数。

```ts
export type SplitDirection = "horizontal" | "vertical";

export type TerminalLayout =
  | { kind: "leaf"; tabId: string }
  | {
      kind: "split";
      direction: SplitDirection;
      ratio: number;
      first: TerminalLayout;
      second: TerminalLayout;
    };
```

约束：

- `ratio` 以 0–1 表示，所有公开变换将其限制在 `0.2–0.8`。
- 一个 tab id 在布局树内最多出现一次。
- 分割只接受终端 tab；Home、Forward、Edit 和串口不进入分栏操作。
- 删除 leaf 后，如果父节点只剩一个子树，直接提升剩余子树，不能留下单子节点 split。
- 删除最后一个 leaf 时布局置空，由 `AppShell` 回到 Home。
- 从 tab 栏切换到某个终端时，若该 tab 已在布局内，激活其 pane；若不在当前布局，则恢复该工作区保存的布局或建立单 leaf。

测试覆盖：

- 新建左右、上下分割；
- 嵌套分割；
- 20% 和 80% 比例边界及越界输入；
- 删除左/右/中间 leaf 后的父节点提升；
- 重复 tab id、未知 tab id 和空布局的拒绝/归一化；
- 激活 leaf 与布局遍历。

## 4. 状态边界

`src/lib/stores/app.svelte.ts` 继续拥有连接 tab 和 active tab；新增的布局状态按工作区保存，但只引用 `tabId`，不复制连接对象。
工作区定义为一个可见的顶层终端 tab；分屏产生的子 tab 仍使用现有 tab/session 机制，但带有 `workspaceId` 和 `paneOf` 元数据，隐藏在顶层 tab 栏中，只作为 pane 的叶子存在。这样可以保留现有 AI、SFTP、session registry 按 tab id 索引，同时避免分屏操作污染顶层 tab 栏。

状态同时维护 `activeWorkspaceId` 和 `activePaneId`：顶层 tab 点击切换工作区并聚焦其根 pane；点击 pane 只更新 active pane，不改变顶层 tab 栏选中项。现有依赖当前终端的 AI、SFTP、搜索和输入路由改用 active pane；导航高亮和窗口标题使用 active workspace。关闭工作区时先关闭其所有子 pane，再移除工作区布局。

`Tab` 类型新增可选的内部 pane 元数据；顶层连接、Home、Forward、Edit 和显式 Clone Tab 不带该元数据，只有同窗分屏创建的子 tab 带它。顶层导航列表必须过滤 pane 子 tab。

建议接口：

- `layoutForWorkspace(workspaceId): TerminalLayout | null`
- `setWorkspaceLayout(workspaceId, layout): void`
- `activePaneId(): string | null`
- `setActivePane(tabId): void`
- `splitPane(tabId, direction, clone): string | null`
- `closePane(tabId): void`

现有 AI、SFTP 和 session registry 继续以 tab id 为索引：

- 新 pane 的连接由现有 clone/connect 流程建立独立 session；
- AI store 仍按新 tab id 建立独立 actor；
- SFTP 继续按 SSH tab 独立保存；
- 关闭 pane 继续走 `ai.disposeTab()` 和 `TerminalPane.onDestroy()`；
- 不新增跨 pane 的隐式输入广播。

如果当前版本没有持久化 workspace id，则第一版以当前主工作区为唯一 workspace，布局只存在内存中；重启恢复不属于本次范围。

## 5. UI 行为

### 5.1 分屏入口

终端 pane 右键菜单提供：

- 向左分屏；
- 向右分屏；
- 向上分屏；
- 向下分屏；
- 在新窗口打开；
- 关闭 pane。

“向左/右/上/下分屏”创建同连接配置的新 tab/session，并立即插入当前 pane 的相邻叶子；它不创建 Tauri 原生窗口，也不触发 `open_tab_in_new_window`。

“在新窗口打开”是严格的独立窗口动作：创建一个新的 Tauri WebView 并克隆当前 tab，不接收分屏方向，不调整当前窗口几何，也不把两个窗口绑定成联动组。前端调用 `open_tab_in_new_window` 时必须使用普通新窗口路径；Rust command 不再承担 split layout 或 window tiling。

### 5.2 Pane 渲染

新增轻量 `TerminalSplitLayout.svelte` 或等价的递归渲染组件：

- leaf 渲染现有 `TerminalPane`；
- split 渲染两个子 pane 和一个可拖拽 separator；
- separator 的方向由 `direction` 决定，命中区域至少 6px；
- 拖拽使用 Pointer Events，拖拽期间更新本地比例，结束时写入布局状态；
- 当前 active pane 添加焦点样式；
- pane header 只显示 label、连接状态和关闭按钮，不复制完整 tab bar。

现有 `AppShell` 的 AI/SFTP side panel 仍位于终端布局外侧。布局树只占用 `main-area`，两侧 panel 的宽度约束继续由现有 `panelFitPriorityByTab` 逻辑处理。

### 5.3 键盘与焦点

- 分屏方向使用右键菜单，避免新增不可见快捷键；
- 终端输入、搜索、粘贴、AI 入口均以当前 active pane 的 tab id 为目标；
- 点击 pane 自动设置 active pane；
- 切换顶层 tab 后，焦点恢复到对应 pane 的 xterm；
- 关闭 active pane 后聚焦其相邻 pane；无相邻 pane 则回到 Home。

## 6. 资源策略

本次先处理已确认的重复运行时开销，再基于可观测数据决定是否进一步收紧 xterm 缓冲。

### 必须实现

- 同窗分栏不能创建新的 Tauri WebView；
- 分栏操作不能启动新的更新轮询、同步轮询或 AppShell 实例；
- xterm `scrollback` 显式设置，避免依赖版本默认值；
- ImageAddon 的 storage/pixel 上限显式保留并集中配置；
- 每个 pane 只注册一次自身的 xterm、session、ResizeObserver 和事件监听；
- pane 销毁时完整执行现有 dispose 路径。

### 观测而非猜测

增加开发期资源快照接口或日志，至少包含：

- 当前 pane 数；
- 每个 terminal 的 cols、rows 和 buffer length；
- ImageAddon 配置上限；
- 当前 WebView 内 TerminalPane 数量。

不把该诊断信息展示给普通用户，也不引入持续定时采样。通过手动复现场景比较改造前后常驻内存，避免仅凭单次快照改变 scrollback 或图片策略。

## 7. 错误处理

- clone/connect 失败时，撤销新 leaf 和对应 tab，原 pane 保持可用，并复用现有 toast 错误展示；
- 分栏树变换失败时不修改现有布局；
- separator 拖拽遇到容器尺寸为 0 或小于最小尺寸时保留旧比例；
- 关闭 pane 与 AI teardown 为异步时，布局先封闭该 tab 的后续操作，继续使用现有 `disposeTab` 代际保护；
- 非桌面和移动端不显示分栏入口；
- 串口 tab 继续禁止 clone/split，因为串口资源独占；
- 纯新窗口 clone 继续使用初始化脚本和独立 WebView；移除仅服务于窗口 split/tiling 的方向参数、几何平铺和窗口联动组逻辑。

## 8. 验收标准

1. 在一个 macOS 窗口打开终端后执行向右分屏，出现两个同窗 pane，不出现第二个原生窗口。
2. 两个 pane 可以连接不同会话；输入只进入获得焦点的 pane。
3. 左右和上下分屏均可用，嵌套分屏比例保持在 20%–80%。
4. 关闭任意 pane 后布局不出现空白 pane 或单子节点 split。
5. 重连、AI 面板、SFTP 和搜索不串到另一个 pane。
6. 明确选择“在新窗口打开”时创建第二个原生窗口；该动作不改变当前窗口大小、不创建 split layout、不绑定窗口拖拽联动。

### 资源

1. 同窗 3 pane 场景的原生窗口数量保持为 1。
2. 不产生重复的更新、同步或 CLI 后台轮询。
3. pane 数量和终端 buffer 诊断值与实际 UI 一致。
4. 在相同连接、相同输出负载下记录改造前后 Activity Monitor/`ps` 常驻内存；报告实际观测值，不设未经测量的硬编码目标。

### 验证

- `npm run test -- src/lib/terminal/layout.test.ts`：布局纯函数行为；
- 现有 Vitest 套件：App store、TerminalPane 相关测试；
- `npm run build`：Svelte/TypeScript 构建；
- 手动 macOS smoke test：单窗口 3 pane、连续输出、重连、AI/SFTP、关闭 pane、移到新窗口；
- 使用 Activity Monitor 或 `ps` 记录同一场景的内存和窗口数量。
