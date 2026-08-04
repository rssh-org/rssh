# 任务 6 报告

状态：已完成
commit：ad773e5（refactor: keep new windows independent）

## 测试 / grep
- `cargo test --manifest-path src-tauri/Cargo.toml commands::window`：通过，2 passed；仅有既有 4 条 warning。
- `cargo check --manifest-path src-tauri/Cargo.toml`：通过；仅有既有 dead-code warnings。
- `git grep -n "open_tab_in_new_window\\|window_groups\\|WindowGroups\\|split_rect\\|compute_split" -- src src-tauri`：仅保留普通 command 定义、注册及前端 clone-only 调用；`window_groups`、`WindowGroups`、`split_rect`、`compute_split` 无结果。

## 文件
- `src-tauri/src/commands/window.rs`：删除 Rect、split/tiling、WindowGroups、SETTLE 及几何/联动测试；command 改为 async `(app, clone)`，保留普通 WebviewWindowBuilder、clone 初始化脚本和 clipboard；新增 clone payload 转义测试。
- `src-tauri/src/state.rs`：删除 AppState.window_groups。
- `src-tauri/src/lib.rs`：删除 Moved 联动处理和 window_groups 初始化；Destroyed 仍调用 close_window_sessions。
- `src-tauri/src/server.rs`、`src-tauri/src/commands/lifecycle.rs`：删除旧 state 初始化。
- `.superpowers/sdd/task-6-report.md`：本报告。

## 自审
- 新窗口不读取 caller window、不接收 split、不调整位置/尺寸、不绑定窗口组；只创建独立 WebView 并注入 clone 字符串。
- clone 脚本继续把 JSON payload 作为 JS 字符串字面量，测试覆盖 JSON 引号、换行和反斜杠转义；clipboard 命令未改动。
- 已确认 Destroyed session 清理路径保留，普通 command 注册保留。

## 疑虑
- cargo check/test 输出的 dead-code warnings 来自既有 AI/CLI 代码，未运行全套测试、formatter 或 lint（遵循任务要求）。
