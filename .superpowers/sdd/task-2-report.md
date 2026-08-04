# 任务 2 报告

状态：DONE_WITH_CONCERNS

代码提交：`eacc433`；审查修复提交：`a607b6a`（fix(terminal): preserve pane side order and MRU）

## TDD RED/GREEN

- RED：先加入 3 个 workspace 行为测试；运行 `npm run test -- src/lib/stores/app.svelte.test.ts`，按预期 32 个测试中 3 个失败，失败原因为 `app.addPane is not a function`。
- GREEN：实现 workspace/pane 状态、布局生命周期、激活与关闭级联后，同一聚焦命令通过：1 个测试文件、32/32 tests passed。

## 修改文件

- `src/lib/stores/app.svelte.ts`
  - `Tab` 增加 `workspaceId`/`paneOf`。
  - 增加 active workspace/pane、按 workspace 布局、导航过滤、布局比例更新、addPane/closePane/关闭 workspace 级联及 AI/SFTP 清理。
  - 顶层 terminal tab 创建 root layout；隐藏 pane 不进入顶层导航；串口禁止分屏。
- `src/lib/stores/app.svelte.test.ts`
  - 增加 3 个基础 workspace/pane 测试，以及四方向叶子顺序和 MRU 回归测试。

## 自审和疑虑

- 自审：布局通过任务 1 的不可变 API 更新；未知 resize path 保持原布局；activeTabId 兼容返回当前 pane；保留既有 MRU、AI dispose 与 SFTP 清理行为。
- 疑虑：四个 side 的顺序通过 store 层对任务 1 `addSplit` 结果做不可变交换；未运行项目级测试、lint 或格式化器（按任务约束）。


## 审查修复追加

- RED：新增四方向顺序与 addPane-MRU 回归测试后，34 个测试中 2 个失败：`right` 叶子顺序仍为 `[child, root]`，且 addPane 未把 workspace root 提到 MRU 前端。
- 修复：在 `addPane` 中对 `right`/`bottom` 交换新 split 的 first/second；新增 pane 激活前调用 `setActiveWorkspace`，保留 MRU 语义；原 right 断言同步为明确的空间方向顺序。
- GREEN：最终运行 `npm run test -- src/lib/stores/app.svelte.test.ts`，1 个测试文件、34/34 tests passed。
- 自审：四个 side 现在分别产生 left/top `[new, target]`、right/bottom `[target, new]`；MRU 开启时 addPane 会移动 workspace root，随后仍激活新 pane。
- 疑虑：仍按约束未运行项目级测试、lint 或格式化器；`a607b6a` 为审查修复提交。