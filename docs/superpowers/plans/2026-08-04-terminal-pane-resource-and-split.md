# 同窗终端分栏与纯新窗口实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 在一个 Tauri WebView 内提供真正的终端分栏，同时将“在新窗口打开”收敛为不带平铺语义的独立新窗口动作。

**架构：** 顶层可见终端 tab 作为 workspace root；同窗分屏创建带 `workspaceId`/`paneOf` 元数据的隐藏子 tab，并由二叉布局树引用。现有 TerminalPane、AI、SFTP 和 session registry 继续按 tab id 工作。新窗口命令只接收 clone payload，删除 split geometry 和窗口联动组。

**技术栈：** Svelte 5 runes、TypeScript、Vitest、xterm.js 6、Tauri 2、Rust unit tests。

---

## 文件与职责

### 新建

- `src/lib/terminal/layout.ts`：无副作用的分栏树类型、构造、插入、删除、比例归一化和遍历。
- `src/lib/terminal/layout.test.ts`：分栏树行为与边界测试。
- `src/lib/components/TerminalSplitLayout.svelte`：递归渲染 workspace 布局、pane header、separator 和焦点状态。

### 修改

- `src/lib/stores/app.svelte.ts`：workspace/pane 元数据、active workspace/pane、布局生命周期、关闭级联、资源快照入口。
- `src/lib/stores/app.svelte.test.ts`：workspace、pane 和关闭级联测试。
- `src/lib/components/AppShell.svelte`：渲染分栏组件、顶层 tab 过滤、焦点路由、分屏菜单、纯新窗口调用。
- `src/lib/components/TerminalPane.svelte`：显式 xterm scrollback、资源快照注册、pane 内部焦点兼容。
- `src/lib/i18n/locales/en.ts`：分屏与纯新窗口菜单文案。
- `src/lib/i18n/locales/zh.ts`：分屏与纯新窗口菜单文案。
- `src-tauri/src/commands/window.rs`：删除 split 参数、几何平铺、WindowGroups 及其测试；保留普通 clone 新窗口和剪贴板命令。
- `src-tauri/src/state.rs`：删除 `window_groups` 字段。
- `src-tauri/src/lib.rs`：删除窗口移动联动事件和 state 初始化。
- `src-tauri/src/server.rs`：删除 headless state 的 `window_groups` 初始化。
- `src-tauri/src/commands/lifecycle.rs`：删除测试 state 的 `window_groups` 初始化。

---

### 任务 1：先实现分栏树纯函数

**文件：**
- 创建：`src/lib/terminal/layout.test.ts`
- 创建：`src/lib/terminal/layout.ts`

- [ ] **步骤 1：编写失败测试，固定树结构和边界契约**

在 `layout.test.ts` 覆盖以下可观察行为：

```ts
import { describe, expect, it } from "vitest";
import {
  addSplit,
  collectLeafIds,
  leaf,
  normalizeRatio,
  removeLeaf,
  type TerminalLayout,
} from "./layout";

const root = leaf("root");

describe("terminal layout", () => {
  it("adds horizontal and vertical siblings", () => {
    expect(addSplit(root, "root", "left", "horizontal", 0.5)).toEqual({
      kind: "split",
      direction: "horizontal",
      ratio: 0.5,
      first: { kind: "leaf", tabId: "left" },
      second: { kind: "leaf", tabId: "root" },
    });
    expect(addSplit(root, "root", "down", "vertical", 0.5)).toMatchObject({
      kind: "split",
      direction: "vertical",
    });
  });

  it("clamps ratios to the usable range", () => {
    expect(normalizeRatio(-1)).toBe(0.2);
    expect(normalizeRatio(1.5)).toBe(0.8);
    expect(normalizeRatio(Number.NaN)).toBe(0.5);
  });

  it("removes a leaf and promotes the remaining child", () => {
    const tree = addSplit(root, "root", "left", "horizontal", 0.5);
    expect(removeLeaf(tree, "left")).toEqual(root);
    expect(removeLeaf(tree, "missing")).toEqual(tree);
  });

  it("supports nested removal without leaving one-child splits", () => {
    const one = addSplit(root, "root", "one", "horizontal", 0.3);
    const two = addSplit(one, "one", "two", "vertical", 0.7);
    expect(collectLeafIds(removeLeaf(two, "one"))).toEqual(["two", "root"]);
  });
});
```

- [ ] **步骤 2：运行测试确认当前失败**

运行：

```bash
npm run test -- src/lib/terminal/layout.test.ts
```

预期：FAIL，模块 `./layout` 尚不存在。

- [ ] **步骤 3：实现最小纯函数模块**

在 `layout.ts` 定义：

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

export const MIN_RATIO = 0.2;
export const MAX_RATIO = 0.8;
export const leaf = (tabId: string): TerminalLayout => ({ kind: "leaf", tabId });
export function normalizeRatio(value: number): number {
  return Number.isFinite(value) ? Math.min(MAX_RATIO, Math.max(MIN_RATIO, value)) : 0.5;
}
```

实现 `addSplit(root, targetId, newId, direction, ratio)`：只在 target leaf 命中且 newId 不在树内时插入；direction 为 horizontal 时 `first=new leaf, second=target` 表示新 pane 在左/上，调用方通过参数顺序控制四个方向。实现 `removeLeaf(root, id)`：未命中返回原树；命中 leaf 返回 `null`；命中 split 后提升非空子树。实现 `collectLeafIds(root)` 深度优先遍历。

- [ ] **步骤 4：运行测试确认通过**

运行：

```bash
npm run test -- src/lib/terminal/layout.test.ts
```

预期：全部布局测试 PASS。

- [ ] **步骤 5：Commit**

```bash
git add src/lib/terminal/layout.ts src/lib/terminal/layout.test.ts
git commit -m "feat: add terminal split layout model"
```

---

### 任务 2：接入 workspace 与 pane 状态

**文件：**
- 修改：`src/lib/stores/app.svelte.ts`
- 修改：`src/lib/stores/app.svelte.test.ts`

- [ ] **步骤 1：为 store 写失败测试**

在 `app.svelte.test.ts` 新增行为测试：

```ts
describe("terminal workspaces", () => {
  it("keeps split children out of the top-level tab list", async () => {
    const app = await loadAppModule();
    app.addTab({ id: "root", type: "local", label: "Root" });
    const child = app.addPane("root", "right", {
      id: "child", type: "local", label: "Child",
    });

    expect(child).toEqual("child");
    expect(app.workspaceTabs().map((tab) => tab.id)).toEqual(["root"]);
    expect(app.paneIdsForWorkspace("root")).toEqual(["child", "root"]);
  });

  it("removes a pane without destroying its sibling workspace", async () => {
    const app = await loadAppModule();
    app.addTab({ id: "root", type: "local", label: "Root" });
    app.addPane("root", "right", { id: "child", type: "local", label: "Child" });
    app.closePane("child");

    expect(app.tabs().map((tab) => tab.id)).toContain("root");
    expect(app.paneIdsForWorkspace("root")).toEqual(["root"]);
  });

  it("closing a workspace closes all hidden pane tabs", async () => {
    const app = await loadAppModule();
    app.addTab({ id: "root", type: "local", label: "Root" });
    app.addPane("root", "right", { id: "child", type: "local", label: "Child" });
    app.closeTab("root");

    expect(app.tabs().map((tab) => tab.id)).not.toContain("root");
    expect(app.tabs().map((tab) => tab.id)).not.toContain("child");
  });
});
```

- [ ] **步骤 2：运行测试确认失败**

运行：

```bash
npm run test -- src/lib/stores/app.svelte.test.ts
```

预期：FAIL，`workspaceTabs`, `addPane`, `closePane` 和 `paneIdsForWorkspace` 尚不存在。

- [ ] **步骤 3：实现状态和生命周期**

在 `Tab` 上增加内部字段：

```ts
export interface Tab {
  id: string;
  type: TabType;
  label: string;
  meta?: Record<string, string>;
  workspaceId?: string;
  paneOf?: string;
}
```

增加状态：

```ts
let _activeWorkspaceId = $state("home");
let _activePaneId = $state("home");
let _layoutByWorkspace = $state<Record<string, TerminalLayout | null>>({});
```

实现并导出：

- `activeWorkspaceId()`、`activePaneId()`；
- `workspaceTabs()`：过滤 Home 以外且没有 `paneOf` 的顶层 tab；
- `layoutForWorkspace(workspaceId)`、`paneIdsForWorkspace(workspaceId)`；
- `setActiveWorkspace(id)`：设置 workspace 和根 pane，调用现有 MRU 逻辑只处理可见 root；
- `setActivePane(id)`：只改变 active pane，拒绝未知或不属于当前 workspace 的 id；
- `isTerminalWorkspace(workspaceId)`：确认当前 workspace root 是终端类型；
- `resizeLayoutPath(workspaceId, path, ratio)`：通过 `normalizeRatio` 更新指定 split 节点，未知 path 不改变布局；
- `addPane(workspaceId, side, tab)`：校验 tab 是终端、workspace 存在、串口禁用；设置 `workspaceId`/`paneOf`，把新 leaf 插入当前 layout；连接失败由调用方回滚；
- `closePane(id)`：从布局删除隐藏 pane，调用现有 `closeTab` 的 AI/session cleanup，但不关闭 workspace root；
- `closeTab(rootId)`：若 root 是 workspace，则先收集并关闭全部子 pane，再删除 root layout；
- `ensureWorkspaceLayout(rootId)`：root 首次加入时创建 `leaf(rootId)`；
- `terminalLayout()`：返回当前 workspace 的布局，供 Svelte 组件响应式渲染。

`setActiveTab` 保留为兼容入口：传入顶层 tab 时调用 `setActiveWorkspace`，传入隐藏 pane 时调用 `setActivePane`。`activeTabId()` 返回 active pane id，新增 `activeWorkspaceId()` 供导航高亮和窗口标题使用。所有原有 `addTab` 的顶层终端连接建立 root layout；显式 Clone Tab 继续创建顶层 root，不使用 `paneOf`。

- [ ] **步骤 4：运行 store 测试确认通过**

运行：

```bash
npm run test -- src/lib/stores/app.svelte.test.ts
```

预期：原有 store 测试和新增 workspace 测试全部 PASS。

- [ ] **步骤 5：Commit**

```bash
git add src/lib/stores/app.svelte.ts src/lib/stores/app.svelte.test.ts
git commit -m "feat: track terminal panes by workspace"
```

---

### 任务 3：加入显式 xterm 配置与开发资源快照

**文件：**
- 修改：`src/lib/stores/app.svelte.ts`
- 修改：`src/lib/components/TerminalPane.svelte`

- [ ] **步骤 1：扩展 TerminalControls 资源接口**

在现有 `TerminalControls` 中增加：

```ts
export interface TerminalResourceStats {
  tabId: string;
  cols: number;
  rows: number;
  bufferLength: number;
  scrollback: number;
  imageStorageLimitMb: number;
  imagePixelLimit: number;
}
```

增加 `readTerminalResourceStats(tabId)` 和 `terminalResourceSnapshot()`，只从已注册 terminal controls 读取快照；未注册 tab 返回 `null`，快照按 tab id 排序，禁止定时采样。

- [ ] **步骤 2：在 TerminalPane 中注册快照读取器**

在 `TerminalPane.onMount` 创建 xterm 前定义常量：

```ts
const TERMINAL_SCROLLBACK = 1000;
const IMAGE_STORAGE_LIMIT_MB = app.isMobile ? 32 : 128;
const IMAGE_PIXEL_LIMIT = app.isMobile ? 4_000_000 : 16_000_000;
```

`new Terminal` 显式传入 `scrollback: TERMINAL_SCROLLBACK`；ImageAddon 使用相同常量；在 `registerTerminalControls` 中加入 `readResourceStats`，读取 `terminal.cols`, `terminal.rows`, `terminal.buffer.active.length` 和上述配置值。销毁时沿用现有 unregister 路径。

- [ ] **步骤 3：加入开发期日志，不引入用户界面**

在 pane 数量或布局更新后，仅当 `import.meta.env.DEV` 为 true 时输出一次：

```ts
console.debug("[rssh] terminal resources", app.terminalResourceSnapshot());
```

生产构建不输出诊断日志。资源快照 API 供 macOS smoke test 和调试工具读取，不新增后台 timer。

- [ ] **步骤 4：运行受影响测试与构建**

运行：

```bash
npm run test -- src/lib/stores/app.svelte.test.ts
npm run build
```

预期：测试 PASS，Vite/Svelte 构建 PASS。

- [ ] **步骤 5：Commit**

```bash
git add src/lib/stores/app.svelte.ts src/lib/components/TerminalPane.svelte
git commit -m "perf: make terminal memory settings explicit"
```

---

### 任务 4：实现递归同窗分栏组件

**文件：**
- 创建：`src/lib/components/TerminalSplitLayout.svelte`

- [ ] **步骤 1：创建递归组件接口**

组件接收当前布局、活动 pane 和回调：

```ts
let {
  layout,
  activePaneId,
  onActivate,
  onResize,
  onClose,
}: {
  layout: TerminalLayout;
  activePaneId: string;
  onActivate: (tabId: string) => void;
  onResize: (path: number[], ratio: number) => void;
  onClose: (tabId: string) => void;
} = $props();
```

每个 leaf 从 `app.tabs()` 查找对应 tab，渲染 pane header 和已有 `TerminalPane`；每个 split 递归渲染 `first`/`second`，通过 `path` 定位比例节点。未知 tab 不渲染空白，而调用布局清理回调在 store 层移除它。

- [ ] **步骤 2：实现 pointer separator**

separator 使用 `pointerdown` 捕获指针，计算相对父容器的比例并调用 `onResize`；比例统一由 `normalizeRatio` 限制。事件清理放在 pointerup/pointercancel；容器尺寸为 0 时不写状态。

- [ ] **步骤 3：实现 pane header 和焦点样式**

header 显示 label、连接状态和关闭按钮。点击 header 或 pane 主体调用 `onActivate(tabId)`；active pane 添加 `active` class。关闭按钮阻止冒泡后调用 `onClose(tabId)`。pane 内部使用 `min-width: 0; min-height: 0; overflow: hidden`，避免嵌套布局撑破主区。

- [ ] **步骤 4：提交组件**

```bash
git add src/lib/components/TerminalSplitLayout.svelte
git commit -m "feat: render terminal panes in one webview"
```

---

### 任务 5：将 AppShell 分屏动作接入并过滤顶层导航

**文件：**
- 修改：`src/lib/components/AppShell.svelte`
- 修改：`src/lib/i18n/locales/en.ts`
- 修改：`src/lib/i18n/locales/zh.ts`

- [ ] **步骤 1：替换顶部导航数据源**

`navSections` 的 tabs 改用 `app.workspaceTabs()`；`isActiveItem` 和 `activateNavItem` 使用 `app.activeWorkspaceId()`。窗口标题使用 active pane 的 terminal title，但 label fallback 使用 active workspace label。现有设置、Home、Forward、Edit 导航保持原语义。

- [ ] **步骤 2：渲染布局而不是为每个 terminal tab 建绝对 pane**

在 `.main-area` 内保留 settings、Home、Forward、Edit 的现有 pane 分支；终端区域改为始终保活当前 workspace 的 `TerminalSplitLayout`，切换 settings/非终端页面只隐藏它，不销毁 TerminalPane：

```svelte
{#if app.layoutForWorkspace(app.activeWorkspaceId())}
  <div class="terminal-layout-pane" class:hidden={app.settingsActive() || !app.isTerminalWorkspace(app.activeWorkspaceId())}>
    <TerminalSplitLayout
      layout={app.layoutForWorkspace(app.activeWorkspaceId())!}
      activePaneId={app.activePaneId()}
      onActivate={app.setActivePane}
      onResize={app.resizeLayoutPath}
      onClose={app.closePane}
    />
  </div>
{/if}
```

不要同时在旧 `{#each app.tabs()}` 中渲染 terminal `TerminalPane`，否则会创建重复 xterm。非终端 tab 继续走旧分支。

- [ ] **步骤 3：把右键菜单分成同窗分屏和纯新窗口**

在 `buildMenu` 中为可分屏终端加入四个方向菜单项，回调调用 `splitCurrentPane(tab, direction)`。该函数创建新的 pane tab 元数据，调用 store 插入布局；connect/创建失败时删除新 tab 并显示现有 toast。

将现有“Open in New Window”子菜单改为无 submenu 的单一菜单项。`openInNewWindow(tab)` 只执行：

```ts
invoke("open_tab_in_new_window", {
  clone: JSON.stringify({ type: tab.type, label: tab.label, meta: tab.meta }),
});
```

不传 `split`，不移动当前窗口，不绑定窗口联动。现有 Clone Tab 快捷键继续创建顶层 workspace，不改成分屏。

- [ ] **步骤 4：更新文案**

在 `en.ts` 和 `zh.ts` 增加四个 split direction label，并将现有 open-new-window 文案明确为“Open in New Window / 在新窗口打开”。删除不再使用的“new window direction”键，避免菜单继续暗示窗口平铺。

- [ ] **步骤 5：补充布局 CSS**

在 AppShell 或组件样式中加入 `.terminal-layout-pane` 的 flex/min-size 规则；不修改现有 AI/SFTP side panel 的宽度算法。验证 `.content`、`.main-area` 和 layout root 在 AI/SFTP 同时打开时仍可收缩。

- [ ] **步骤 6：运行前端验证**

运行：

```bash
npm run test
npm run build
```

预期：Vitest 全部 PASS，构建 PASS；静态检查不得出现未使用的 split window helper 或 i18n key。

- [ ] **步骤 7：Commit**

```bash
git add src/lib/components/AppShell.svelte src/lib/i18n/locales/en.ts src/lib/i18n/locales/zh.ts
git commit -m "feat: split terminals inside the active window"
```

---

### 任务 6：让 Rust 新窗口严格保持普通新窗口

**文件：**
- 修改：`src-tauri/src/commands/window.rs`
- 修改：`src-tauri/src/state.rs`
- 修改：`src-tauri/src/lib.rs`
- 修改：`src-tauri/src/server.rs`
- 修改：`src-tauri/src/commands/lifecycle.rs`

- [ ] **步骤 1：收紧 command 签名**

将 command 从 `open_tab_in_new_window(app, window, clone, split)` 改为 `open_tab_in_new_window(app, clone)`；保留 async。删除 `Rect`、`split_rect`、`compute_split`、`place`、`WindowGroups`、`SETTLE` 和所有 directional tiling 分支。普通路径只构建独立 WebView：

```rust
#[tauri::command]
pub async fn open_tab_in_new_window(app: AppHandle, clone: String) -> AppResult<()> {
    let json_literal = serde_json::to_string(&clone).map_err(|e| {
        AppError::other("window_clone_encode_failed", serde_json::json!({ "err": e.to_string() }))
    })?;
    let init_script = format!("window.__rssh_clone = {};", json_literal);
    let label = format!("rssh-{}", Uuid::new_v4().simple());
    WebviewWindowBuilder::new(&app, &label, WebviewUrl::App("index.html".into()))
        .title("RSSH")
        .inner_size(1200.0, 800.0)
        .initialization_script(&init_script)
        .build()
        .map_err(|e| AppError::other("window_open_failed", serde_json::json!({ "err": e.to_string() })))?;
    Ok(())
}
```

- [ ] **步骤 2：删除窗口联动状态与事件**

从 `AppState`、桌面 `lib.rs` setup、headless `server.rs` 和 lifecycle `empty_state` 删除 `window_groups` 字段/初始化。删除 `on_window_event` 中 `Destroyed` 的 group remove 和 `Moved` 的 sibling reposition 分支；`Destroyed` 仍调用 `close_window_sessions`。

- [ ] **步骤 3：删除过时 Rust 测试**

从 `window.rs` 测试模块删除 split geometry 和 WindowGroups 测试；保留并新增普通 command 的 payload 编码测试，验证 clone 字符串能生成 `window.__rssh_clone = ...;`，不包含 split/position/size 联动参数。

- [ ] **步骤 4：更新所有调用点并运行 Rust 测试**

搜索确认只剩普通调用：

```bash
git grep -n "open_tab_in_new_window\|window_groups\|WindowGroups\|split_rect\|compute_split" -- src src-tauri
cargo test --manifest-path src-tauri/Cargo.toml commands::window
```

预期：调用点只有 `invoke("open_tab_in_new_window", { clone })` 和 command 注册；`window_groups`、`WindowGroups`、旧 split helper 搜索无结果；窗口模块测试 PASS。

- [ ] **步骤 5：Commit**

```bash
git add src-tauri/src/commands/window.rs src-tauri/src/state.rs src-tauri/src/lib.rs src-tauri/src/server.rs src-tauri/src/commands/lifecycle.rs
git commit -m "refactor: keep new windows separate from pane splits"
```

### 任务 7：端到端验证与资源观察

**文件：**
- 修改：必要时仅修复前述任务暴露的测试/类型问题；不增加无关重构。

- [ ] **步骤 1：运行完整前端与 Rust 验证**

```bash
npm run test
npm run build
cargo test --manifest-path src-tauri/Cargo.toml
```

预期：三条命令均成功；任何失败都必须定位到具体改动后修复，再重新运行对应命令。

- [ ] **步骤 2：执行 macOS 同窗分栏 smoke test**

启动桌面应用，执行以下场景并记录结果：

1. 打开一个本地或 SSH 终端。
2. 右键选择向右分屏，确认 macOS 只显示一个 RSSH 原生窗口。
3. 在两个 pane 分别输入命令，确认输出不会串 pane。
4. 再执行上下分屏，拖动两条 separator，确认比例不低于 20% 且不高于 80%。
5. 切换 AI/SFTP、切换顶层 workspace、关闭中间 pane，确认剩余 pane 和 session 正常。
6. 明确选择“在新窗口打开”，确认出现第二个独立窗口，原窗口大小不变，两个窗口不跟随拖动。

- [ ] **步骤 3：记录资源快照**

在同一输出负载下分别记录改造前后：

```bash
ps -axo pid,rss,command
```

从输出中记录 RSSH 进程的 RSS、当前原生窗口数量和 `app.terminalResourceSnapshot()` 的 pane/buffer 数；不写未经观察的固定目标。

- [ ] **步骤 4：最终检查**

```bash
git status --short
git log -7 --oneline
```

预期：工作区干净；最终提交信息分别表达布局模型、store 接入、资源配置、UI 接入和 Rust 新窗口边界。

---

## 计划自检

- 规格覆盖：布局树由任务 1 覆盖；workspace/pane 状态和关闭级联由任务 2 覆盖；xterm 内存配置和资源快照由任务 3 覆盖；递归 UI、separator、焦点由任务 4 覆盖；顶层导航、菜单、同窗分屏和纯新窗口调用由任务 5 覆盖；Rust command、state、事件和过时联动逻辑由任务 6 覆盖；行为、内存和窗口数量验收由任务 7 覆盖。
- 质量检查：所有步骤均有具体文件、接口、代码片段、命令和预期结果，没有临时标记或未定义的实现动作。
- 类型一致性：`TerminalLayout`、`SplitDirection`、`workspaceId`、`paneOf`、`activeWorkspaceId`、`activePaneId` 与规格和前序任务保持同名；布局组件只调用任务 2 定义的 store API。
- 范围检查：前端布局、store、Rust 纯新窗口和验证属于同一可工作的垂直切片；不引入后端线程池或无关性能重构。
