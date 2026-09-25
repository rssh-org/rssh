# rssh 本地定制（基于官方 v0.3.1 @ 0a7401d）

本仓库是官方 rssh 的本地改造版，基线为官方 **v0.3.1**（commit `0a7401d`）。
所有定制累积在版本线 v0.0.2 → v0.0.13，交付物为 `out/portable/rssh-v0.0.13.exe`。

## 为什么本地 git 仓库没有官方历史

本机网络无法直连 github.com:443（git fetch 被 reset / 无法连接）。
因此本仓库只含**当前定制后的快照**，不包含官方历史。
若以后网络可用（代理/VPN），可执行：

```
git remote add upstream https://github.com/rssh-org/rssh.git
git fetch --depth 250 upstream main
git merge upstream/main          # 或：git rebase upstream/main
```

## 定制功能版本线

| 版本 | 内容 |
| --- | --- |
| v0.0.2 | 远程目录 symlink 跟随 + 图片/PDF 预览 |
| v0.0.3 | 独立编辑窗口 + 表格编辑器 |
| v0.0.4 | 表格表头冻结 |
| v0.0.5 | 预览/编辑内嵌为标签页（VS Code 式） |
| v0.0.6 | 编辑未保存退出确认 |
| v0.0.6.1 | 文件加载超时提示 |
| v0.0.7 | SFTP 超时处理 |
| v0.0.7.1 | GPU 禁用（WebView2 闪退修复）+ 错误日志 |
| v0.0.8 | 死循环根治 + 心跳看门狗 |
| v0.0.9 | 已打开文件高亮 + 图片与终端并排 |
| v0.0.10 | 文件列表排序（名称/大小/时间，正倒序） |
| v0.0.11 | Pi 编码代理会话内嵌（TRAE 式）+ 会话历史选择器 + 会话标题/删除 + SSH config 一键导入 + Pi 标签带服务器名 |
| v0.0.12 | **OpenCode 会话内嵌**（openchamber/TRAE 式）：SSH 标签右键「打开 OpenCode 会话」→ 远端自动起 `opencode serve`（39801，Basic Auth 密码持久化）+ 本地端口转发 + 事件总线流式；聊天页含会话列表/新建/删除/中止、消息历史渲染（文本/思考/工具卡片） |
| v0.0.13 | **性能线**：① 恢复 GPU 渲染（删 `--disable-gpu`，修 v0.0.7.1 起的全局软件渲染卡顿；闪退逃生门见下）② 终端输出 IPC 改 base64（原 JSON 数字数组膨胀 4x）③ SSH 输出 8ms/64KB 攒批 ④ PTY/Telnet 读缓冲 4KB→64KB ⑤ 修复 v0.0.11 起坏掉的 cargo test 与 headless 构建（AppState 初始化缺 `pi_sessions`/`opencode_sessions`：测试夹具 lifecycle.rs/plugin.rs + `server.rs build_state`；Windows 测试二进制缺 comctl32 v6 manifest → 新增 `scripts/run-tests.sh`） |

## v0.0.13 说明

- **GPU 逃生门**：若 WebView2 闪退复发，启动前设环境变量 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--disable-gpu` 即退回软件渲染，无需重编译（WebView2 原生追加参数机制）。
- **数据事件 wire format 变更**：`ssh:data` / `pty:data` / `telnet:data` / `serial:data` 的 payload 从 `number[]` 改为 base64 字符串（`emitter::b64_payload` 统一编码，桌面与 headless 同构）。前端统一走 `src/lib/terminal/stream-decode.ts` 解码（原生 `Uint8Array.fromBase64` 优先，atob 兜底）。
- **SSH 攒批**：`session_task` 缓冲输出，8ms 窗口或 64KB 满即 flush；EOF/Close 前与下次 Write 前强制 flush，保序不丢尾。
- **Windows 跑 Rust 测试**：用 `bash scripts/run-tests.sh`（直接 `cargo test` 会因 comctl32 v6 manifest 缺失起不来，见脚本头注释）。

## 改动文件清单（相对官方 v0.3.1）

### 新增文件

- `src/lib/components/PiSessionTab.svelte` — Pi 聊天页（RPC 流式消息/思考/工具卡片、get_messages 历史渲染、会话选择器、删除会话）
- `src-tauri/src/commands/pi.rs` — Pi 会话命令（start/send/stop/list/delete）
- `src/lib/components/OpencodeSessionTab.svelte` — OpenCode 聊天页（HTTP + SSE 事件流；会话列表/新建/删除/中止；消息历史渲染文本/思考/工具卡片；同步 `/message` 发送）
- `src-tauri/src/commands/opencode.rs` — OpenCode 会话命令（start：远端起 serve + 本地转发 + 事件流线程；stop/list/new/delete/send/messages/abort）
- `src-tauri/src/ssh/pi.rs` — PiHandle 持久进程（spawn `pi --mode rpc`、`pi:line:{id}` / `pi:status:{id}` 事件）
- `src/lib/terminal/stream-decode.ts`（v0.0.13）— 数据事件 base64 → Uint8Array 解码（原生 fromBase64 优先）
- `scripts/run-tests.sh`（v0.0.13）— Windows 下跑 cargo test 的包装（内嵌 comctl32 v6 manifest）
- `src-tauri/windows-test.manifest`（v0.0.13）— 上条脚本引用的测试 manifest

### 修改文件（重新应用时对照）

- `src/lib/stores/app.svelte.ts` — TabType 加 `pi_session`；`openPiSession(sshTabId)` 标签标题带服务器名（`4L · Pi`）；TabType 加 `opencode_session`；`openOpencodeSession(sshTabId)`（`4L · OpenCode`）
- `src/lib/components/AppShell.svelte` — 渲染 Pi 会话标签 + OpenCode 会话标签；SSH 标签右键菜单加「打开 Pi 会话」「打开 OpenCode 会话」
- `src/lib/i18n/locales/zh.ts` / `en.ts` — 新增 `pi.*`、`home.import.*`、`tab.context.opencode` 等 key
- `src-tauri/src/commands/profile.rs` — 追加 `ssh_config_scan` / `ssh_config_import`（从 ~/.ssh/config 导入主机与私钥凭据）
- `src-tauri/src/commands/mod.rs` — 声明 `pi`、`opencode` 模块
- `src-tauri/src/ssh/mod.rs` — 声明 `pi` 模块
- `src-tauri/src/state.rs` — `pi_sessions` map；`opencode_sessions` map
- `src-tauri/src/lib.rs` — 注册 6 个 pi 命令 + 8 个 opencode 命令 + 2 个 ssh_config 命令；AppState 初始化
- `src-tauri/src/emitter.rs`（v0.0.13）— `b64_payload()` + `Host::emit_bytes()`：终端字节流统一 base64 编码
- `src-tauri/src/ssh/client.rs`（v0.0.13）— `session_task` 输出攒批（8ms/64KB）+ `emit_bytes`
- `src-tauri/src/commands/{pty,serial,telnet}.rs` + `src-tauri/src/server.rs`（v0.0.13）— 四条数据事件改 base64；`server.rs build_state` 补缺失 AppState 字段（修复 headless 构建）
- `src-tauri/src/terminal/{pty,telnet}.rs`（v0.0.13）— 读取缓冲 4KB→64KB
- `src-tauri/src/commands/{lifecycle,plugin}.rs`（v0.0.13）— 测试夹具 `empty_state`/`test_state` 补缺失字段（修复 cargo test 编译）
- `src/lib/components/TerminalPane.svelte` + `src/lib/ai/store.svelte.ts`（v0.0.13）— 数据事件监听改 base64 解码
- `src-tauri/tauri.conf.json`（v0.0.13）— 删除 `additionalBrowserArgs: --disable-gpu`，恢复 GPU

## 构建

```
npm run build
npm test                       # 799 项
bash scripts/run-tests.sh      # cargo test 737 项（Windows 必须，见脚本注释）
cd src-tauri && cargo build --release --features "cli,custom-protocol"
# 产物：src-tauri/target/release/rssh.exe → 拷贝到 out/portable/rssh-v0.0.13.exe
```

## OpenCode 集成说明（v0.0.12）

- 远端 serve：rssh 在 SSH 会话上检查 `opencode serve`（固定端口 **39801**，避开 TRAE 的 39797）是否在跑；没跑则 `OPENCODE_SERVER_PASSWORD=<uuid> nohup opencode serve ... &` 启动。密码持久化在 `~/.config/rssh-opencode/pass`（0600），重启 rssh 后复用同一 serve。
- Basic Auth：用户名固定 `opencode`，密码为上面持久化的值（opencode 官方约定）。
- 数据：会话存服务器 `~/.local/share/opencode/opencode.db`（SQLite）→ 换机连同一台主机可续接同一会话。
- 发送走同步 `POST /session/:id/message`（1.18 实测可靠）；`/event` SSE 事件流并行监听（session.idle / error 状态）。
- 若 rssh 起的 serve 默认 model 不可用（如 403 余额不足），在服务器上配好 opencode 默认 provider：`opencode models` 或 `~/.config/opencode/opencode.json`。

## 升级衔接（官方发新版时）

1. 获取官方新版 zip（本机 GitHub 直连不通时用镜像 / 代理 / 浏览器下载）。
2. 解包后，按上面「改动文件清单」把**新增文件**复制进去，对**修改文件**逐点重放。
3. 重新 `npm run build` + `cargo build --release --features "cli,custom-protocol"`。
4. 跑一遍冒烟（打开 exe、连 4L、开 Pi 会话、看会话历史、导入 config）。

> 若之后本机能连 GitHub，把官方历史 fetch 进来后，本清单就是 merge 冲突时的对照依据。
