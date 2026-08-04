# 任务 3 报告

状态：已完成
commit：c49d22a（feat: add terminal resource snapshots）

## 测试结果
- `npm run test -- src/lib/stores/app.svelte.test.ts`：通过，34 tests passed。
- `npm run build`：通过，Vite production build 完成。
- 构建仍报告既有 AppShell 可访问性提示及 chunk 体积提示；本任务未修改相关路径。

## 修改文件
- `src/lib/stores/app.svelte.ts`：新增 `TerminalResourceStats`、资源读取 API、稳定排序快照，并扩展 `TerminalControls`。
- `src/lib/components/TerminalPane.svelte`：显式 scrollback/image 配置、资源读数注册及 DEV-only 注册后 debug 日志；保留原注册/销毁路径。

## 自审
- 未注册或未提供资源读数的 tab 返回 `null`，快照仅包含可读 tab 并按 `tabId` 排序。
- 图片存储/像素限制与 ImageAddon 使用同一组移动端/桌面常量，bufferLength 直接读取活动 buffer。
- 未增加 timer、UI 或 AppShell/Rust 改动；生产构建不会输出该 debug 日志。

## 疑虑
- 仅按任务要求运行聚焦 store 测试与 build，未运行全套测试、lint 或 formatter。
