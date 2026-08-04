# 任务 5 报告

状态：已完成
commit：85a5d93（实现提交；本报告随后单独提交）

## 构建结果
- `npm run build`：通过（Vite production build，exit 0）。
- 输出仅保留既有 a11y 与 chunk-size warnings；本任务未运行项目级测试套件、lint 或 formatter。

## 修改文件
- `src/lib/components/AppShell.svelte`
  - 顶层导航、键盘跳转、拖拽和高亮改用 workspaceTabs/active workspace；终端功能改用 active pane。
  - 以一个保活的 TerminalSplitLayout 替换旧 terminal loop；保留 settings/Home/Forward/Edit 路由，并增加布局 root 尺寸约束。
  - 接入四方向同窗分屏、失败 rollback/toast、pane 激活/关闭；新窗口仅传 clone 且移除方向 submenu。
- `src/lib/i18n/locales/en.ts`
- `src/lib/i18n/locales/zh.ts`
  - 增加四个 split direction 文案并移除旧 new-window direction 文案。

## 自审
- app.addPane 负责把新 pane 写入 workspace/pane 元数据；AppShell 只生成无 pane 元数据 clone，失败时清理已写入 pane。
- 单个 TerminalSplitLayout 在切换 settings/非终端路由时仅隐藏，active pane header/body 激活后 AI/SFTP 跟随。
- 顶层排序仅接受 workspaceTabs，隐藏 pane 不进入导航、快捷键或关闭入口。

## 疑虑
- TerminalPane 组件已提供一次性 initial failure callback；reconnect 失败继续由既有 UI 处理，store API 本身仍不暴露异步连接失败回调，AppShell 只移除 hidden pane。


## 复审修复
- 修复 commit：0507b0a（本报告更新随后单独提交）。
- `TerminalSplitLayout.svelte` 的 leaf 将真实 tabId 传给 pane-specific context callback；AppShell 不再从父级 activePaneId 推断右键目标。
- `TerminalPane.svelte` 对 serial/telnet/local/SSH 初次连接失败统一执行一次性 callback；reconnect 明确关闭 initial 标记，因此重连失败不会删除既有 pane。
- AppShell 对失败 hidden pane 调用 `closePane` 并复用 `toast.error(errMsg(error))`；root pane 只提示，不删除。
- 复审验证：`npm run build` 通过（exit 0）；仍仅有既有 a11y/chunk-size warnings，未运行整套测试、lint 或 formatter。

- 后续修复 commit：7f68016，恢复 TerminalPane 的 connectGeneration/destroyed/disconnected 生命周期状态声明；修复后 `npm run build` 仍 exit 0。

- 后续修复 commit：d1bfaf5，恢复 onMount 中 `const generation = connectGeneration + 1`；修复后 `npm run build` exit 0。

- 后续修复 commit：545891f，TerminalSplitLayout leaf 增加 `{#key tab.id}`，跨 workspace 复用布局位置时确保 TerminalPane 按真实 tab 身份销毁/重建；修复后 `npm run build` exit 0。

- 后续修复 commit：b072907，恢复 Home/Forward/Edit 非终端 pane 的真实 tab contextmenu（clone/close），并保留 terminal leaf 的 pane-specific handler；修复后 `npm run build` exit 0。

- 后续修复 commit：cce58f2，恢复 Telnet 初次连接失败后的 `Press any key to reconnect.` 提示，保留 root pane 的可见重连入口；修复后 `npm run build` exit 0。

- 后续修复 commit：fe9d945，维护 newPaneId→sourcePaneId pending map；初次失败 closePane 后仅当 source 仍在当前 workspace layout 时恢复 active pane，成功连接/正常关闭自动清理 pending；静态 grep 与 `npm run build` 均通过。