# 任务 2 报告

状态：DONE_WITH_CONCERNS

代码提交：`eacc433`（feat(terminal): connect workspace pane state）

## TDD RED/GREEN

- RED：先加入 3 个 workspace 行为测试；运行 `npm run test -- src/lib/stores/app.svelte.test.ts`，按预期 32 个测试中 3 个失败，失败原因为 `app.addPane is not a function`。
- GREEN：实现 workspace/pane 状态、布局生命周期、激活与关闭级联后，同一聚焦命令通过：1 个测试文件、32/32 tests passed。

## 修改文件

- `src/lib/stores/app.svelte.ts`
  - `Tab` 增加 `workspaceId`/`paneOf`。
  - 增加 active workspace/pane、按 workspace 布局、导航过滤、布局比例更新、addPane/closePane/关闭 workspace 级联及 AI/SFTP 清理。
  - 顶层 terminal tab 创建 root layout；隐藏 pane 不进入顶层导航；串口禁止分屏。
- `src/lib/stores/app.svelte.test.ts`
  - 增加 3 个 workspace/pane 行为测试。

## 自审和疑虑

- 自审：布局通过任务 1 的不可变 API 更新；未知 resize path 保持原布局；activeTabId 兼容返回当前 pane；保留既有 MRU、AI dispose 与 SFTP 清理行为。
- 疑虑：`addPane` 的 side 目前按 left/right 与 top/bottom 映射方向，沿用任务 1 `addSplit` 的固定新叶顺序；未运行项目级测试、lint 或格式化器（按任务约束）。
