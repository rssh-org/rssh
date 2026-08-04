# 任务 5 报告

状态：已完成
commit：5be66d0

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
- TerminalPane 的后端连接失败仍由其既有 reconnect UI 处理；AppShell 能对 addPane 同步失败执行 rollback/toast，但 store API 没有异步连接失败回调。
