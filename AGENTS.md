# AGENT.md — RSSH 仓库导航

> **AGENT.md 是导航，不是真实来源；与代码冲突时以代码为准。**
> 行号、数量、签名都会变；不要把这里的字面量复制进 PR。每条声明都给了**核对命令**，自己跑一遍再用。
> 所有方案，必须是行业规范，不能走偏门，歪门邪道必须跟用户沟通后。

---

## 置顶规则（其他 AI 必读，违反就回滚）

这些是反复踩出来的硬约束。不是建议。

### R1. 后端事件命名：`<domain>:<event>:<instanceId>`

不带实例 ID 后缀的全局事件会在多 tab / 多传输并存时互相串话。第三段按事件实际作用域使用 session、prompt 或 transfer ID；不要一律假定它是 tab ID。

```bash
rg 'emit\(|format!\("[a-z]+:' core/src app/src   # 核对最终 channel 的构造位置
```

### R2. CLI ↔ GUI 走 OSC 7337，不要再造 IPC

GUI 内嵌终端跑 `rssh-cli` 时，CLI 通过 OSC 7337 转义序列与 GUI 通信，xterm parser 解码后调 store。要扩通信，加个 OSC kind，**不要**起 socket / pipe / tauri event。

```bash
rg 'OSC_RSSH_ID|7337' frontend cli/src
```

### R3. 新增前端 command 必须检查两个后端适配器

共享逻辑放在 `core/src/commands/`，Tauri wire wrapper 放在 `app/src/tauri/commands/` 并加入 `app/src/tauri/mod.rs` 的 `generate_handler!`。同一前端功能若要在 JetBrains / browser headless 模式可用，还必须在 `app/src/server/mod.rs` 的 dispatcher 中暴露同名 command，并尽量复用同一 domain helper。漏任一目标适配器，那个目标运行时就是 "command not found"。

```bash
rg 'generate_handler!' app/src/tauri/mod.rs
```

### R4. Tab 根容器必须明确尺寸与滚动所有权

`.pane.visible` 是 `position: absolute; inset: 0; display: flex; flex-direction: column`。直接子树必须填满它，并明确由哪一层滚动。普通滚动页面沿用 HomeScreen 的三件套：

```css
flex: 1;
overflow-y: auto;
min-height: 0;   /* 缺这条 flex 子元素不收缩，overflow 失效 */
```

终端、编辑器、Forward 这类固定画布可以用 `height: 100%`，把 overflow 留给内部 xterm / editor / list；不要盲目给根节点加 `overflow-y: auto`。共同不变量是：flex 子项需要收缩时必须有 `min-height: 0`，而且滚动只能有一个清晰 owner。

### R5. Secret 不进 DB 明文，只走统一的加密存储

调用方只能走 `SecretStore` (`core/src/secret/`)。`HybridStore` 用 ChaCha20-Poly1305 加密后把密文写入 DB `secrets` 表。首次选择 backend 时，系统 keychain 可用就把 master key 放 keychain，否则（包括 Android 和部分 headless 环境）放 data dir 下的 `master.key`；选择结果是 sticky 的，已选 keyring 后 keychain 失效必须硬失败，不能静默换新 file key。不要直接读写 `secrets` 表，也不要把原 secret 当成 keychain value 的当前架构。

```bash
rg 'secret_store|SecretStore' core/src app/src
```

### R6. 不要建"分析文档" / "实施计划归档"

工作流要求临时 `IMPLEMENTATION_PLAN.md` / `DESIGN.md` 等文件时，用完即删，不要把过程记录沉淀下来；除非用户明确要长期文档。

### R7. Svelte 5 runes only

`$state` / `$derived` / `$effect` / `$props`，事件 `onclick={fn}`。看到 `$:` / `export let` / `on:click` ——拒绝合并，让作者升级。

### R8. 跨页 state 放进对应 store，不放组件单例

Tab、导航和连接会话协调在 `app.svelte.ts`；AI、主题、快捷键、传输等分别在既有 domain store。沿用私有 `$state` + 导出 getter / action 的边界，**不要**导出裸 `$state` 对象，**不要**在组件里新建跨页全局状态，也不要把所有领域硬塞回 `app.svelte.ts`。

```bash
rg '^let _.*\$state|^export function' frontend/lib/stores frontend/lib/ai frontend/lib/themes
```

### R9. 布局、输入方式与宿主能力分别判断

页面排版只看窗口或容器宽度：CSS 用 media/container query，需要切换 DOM 时用 `layout.compact()`；不能用 UA、OS 或触摸能力选择手机/电脑布局。触摸、鼠标、键盘按实际输入事件处理，窄窗口不代表没有硬件键盘，宽窗口也可能需要软键盘。

原生功能必须走 `app.capabilities()`（后端 `get_runtime_capabilities`），不能从宽度推断 PTY、串口、多窗口、目录授权等能力。Rust 只在平台 API、库或实现不同处使用 `#[cfg(...)]`。OHOS 的 Rust 目标同时是 `target_os="linux"` 和 `target_env="ohos"`；Linux 专用的 libudev、Secret Service、WebKitGTK 等必须排除 OHOS；Rust 源码统一使用 `linux` / `macos` / `windows` / `android` / `ios` / `ohos` 六个互斥条件（`windows` 为 Rust 内建，其余由三个 crate 的 build.rs 共用 scripts/platform-cfg.rs 定义）；`linux` 明确排除 OHOS，不使用 Tauri 的 `mobile` / `desktop` 分类。多系统实现显式列举，例如 `any(android, ios)`。鸿蒙两个 HAP 共用一份 Rust 库，手机/平板与电脑由 `ohos::device::DeviceClass` 在运行时区分，不能伪造两个编译期 OS。Cargo.toml 依赖条件仍使用原生 target 表达式（build.rs 的 cfg 不参与依赖解析）。App 成功加载能力后才挂载资源页面。Docker CLI、kubectl、keychain 等外部能力仍应在运行时真实探测。

```bash
rg 'layout\.compact|@media|@container|capabilities\(' frontend
cat scripts/platform-cfg.rs  # 三个 crate 共用的六系统 cfg 定义
rg 'cfg\(|cfg!\(' core/src app/src cli/src ohos/src
```

### R10. 新增功能必须显式考虑四条入口

每个新 feature / UI 改动，PR 描述里至少写清以下入口各自怎么处理：

- **桌面 GUI**：默认目标，必须可用
- **移动 GUI**：窄窗口与触摸输入都要可操作，同时保留外接键盘/鼠标的能力；宽屏平板使用宽布局。操作需有可点击入口；确实缺少原生 API 的功能由能力查询关闭，不能按设备名称一概隐藏。
- **CLI**：`cli/src/`。CRUD 类操作大概率要补；纯 UI/可视化类可声明 N/A
- **Headless / JetBrains**：`app/src/server/mod.rs` + `frontend/lib/ipc-shim.ts`。复用同一前端，但 command / event 需要 server adapter 支持

允许的结论是“全部支持”或“只在 X 入口，因为 Y”。**不允许**的是没想过——上线后才发现移动端按钮够不着、CLI 改了 schema 但读不出新字段、JetBrains 页面只能报 unknown command。

```bash
rg 'layout\.compact|@media|@container' frontend/lib/components  # 看宽度响应式布局
rg '#\[cfg\(' core/src/commands   # 看 command 层平台分支
rg '"[a-z_]+" =>' app/src/server/mod.rs  # 看 headless dispatcher
```

### R11. Transport session 必须经过 lifecycle registry

SSH / PTY / serial / Telnet / SFTP / forward 的 open 路径必须先在 `commands/lifecycle.rs` 预留规范 UUID、绑定 `SessionOwner` 与 nonce，再把 Ready handle 激活进 typed map。取消、关闭、窗口销毁和 reload reconcile 都依赖这份 registry；不要绕过它直接向 `sessions` / `pty_sessions` 等 map 插 handle。

```bash
rg 'reserve_resource|\.activate\(|reconcile_owner|close_owner' core/src/commands app/src/server/mod.rs
```

### R12. 动态发现只持久化 source，不持久化结果

`dynamic_discovery_sources` 是用户配置，进入 DB 与 sync；Docker container / K8s pod 的发现结果是瞬时 launch target，只能转成 connector-backed PTY tab。不要偷偷写成 Profile，也不要让已消失的容器在 Home 留陈旧数据。

```bash
rg 'dynamic_discovery_sources|DynamicDiscoveredTarget|connectDynamicTarget' frontend core/src app/src
```

### R13. Github/WebDav 配置同步的兼容规则
如果修改了同步的payload，需要保证新客户端运行没有任何问题，比如旧配置是：{a,b,c,d}，新配置是：{a, b: {c, d}}，新客户端在第一次启动或者拉取旧配置是，需要将c,d映射到b里面
而旧客户端，使用新配置，如果有问题不用管，即新客户端生成的配置是：{a, b: {c, d}}，而不是{a, b: {c, d}, c, d}

### R14. 历史迁移 SQL 是冻结的，永远不改
`schema.rs` 里 `version < N` 的块（建表 / ALTER / 旧 seed）已经跑过就永远保持原样 —— 不要改低版本的 DDL，不要在旧版本块里加列或换 seed。
同一个 `version` 在不同时间产出两种表结构，等于让迁移不再是版本的纯函数。要改默认数据或表结构，就开新版本块。

```bash
rg 'if version <' core/src/db/schema.rs   # 每个块只加不改
```

---

## 事实（Facts）— 文件 + 概念 + 核对命令

### 二进制与 crate 布局

| 概念 | 在哪 | 怎么验证 |
|---|---|---|
| GUI 二进制 `rssh` | `app/src/main.rs` | `cat app/Cargo.toml` 看 `[[bin]]` |
| CLI 二进制 `rssh-cli` | `cli/src/main.rs` + `commands/`，独立 crate | `cat cli/Cargo.toml`；直接依赖 `rssh-core` |
| Headless 二进制 `rssh-server` | `app/src/server_main.rs`，gated by feature `server` | 同上；给 browser / JetBrains 提供 embedded HTTP + WebSocket IPC |
| 共享引擎 `rssh_core` | `core/src/lib.rs`，不依赖 Tauri | `cargo tree -p rssh-core` |
| 应用 lib `rssh_lib` | `app/src/lib.rs`，Tauri / headless 适配入口 | `cat app/Cargo.toml` 看 features |

### 平台源码与构建产物

| 概念 | 在哪 | 怎么验证 |
|---|---|---|
| Android 原生工程 | `android/` | `npm run tauri -- android dev --open` 先准备生成工程再打开 IDE |
| OHOS 平台实现 | `ohos/app/`、`ohos/src/` | ArkTS 工程与 Rust 适配集中维护；核对 `app/src/lib.rs` 模块入口 |
| OHOS 原生适配测试 | `ohos/tests/` | `npm test` 直接测试 `ohos/app/` 源码，不依赖生成目录 |
| OHOS 框架与准备工具 | `ohos/framework/`、`ohos/scripts/` | 独立 lock、上游 revision、patch、测试及生成 workspace |
| 图标 | `icons/` | `app/tauri.conf.json` 的 `bundle.icon` 引用；移动端复制进生成工程 |
| 桌面打包准备 | `desktop/scripts/` | 编译 CLI 后放入 app/bin，再交给 Tauri bundler |
| iOS 资源 | `ios/` | Info.plist / PrivacyInfo 随打包进入生成项目 |
| 平台构建入口 | `scripts/build-*.sh`、`scripts/tauri.mjs` | `npm run tauri -- ...` 调用仓库准备流程，勿直接绕过 wrapper |
| 生成目录 | `app/gen/`、`app/bin/`、`target/` | 全部忽略；源码修改必须回到对应平台目录 |

`app/capabilities/` 是权限源码，保留跟踪。macOS / Windows / Linux 打包时
统一编译 CLI、暂存到 `app/bin/` 并装入应用资源，供 GUI 安装。
普通开发和 Cargo 检查不依赖预编译二进制。各平台构建与打包见
[CONTRIBUTING.md](CONTRIBUTING.md#build)。

### 前端

| 概念 | 在哪 | 怎么验证 |
|---|---|---|
| Tab / 导航 / 会话协调 store | `frontend/lib/stores/app.svelte.ts` | 搜 `export function` |
| 领域 store | `frontend/lib/{ai,themes}/` + `frontend/lib/stores/` | AI、主题、快捷键、传输等各自维护 |
| Tab 渲染分发 | `frontend/lib/components/AppShell.svelte` | 搜 `tab.type === ` |
| 终端层 | `frontend/lib/components/TerminalPane.svelte` | 单文件，xterm + 高亮 + auth 全在内 |
| OSC 解码 | `frontend/lib/osc/handler.ts` | `registerRsshOscHandlers` |
| Headless IPC shim | `frontend/lib/ipc-shim.ts` | Tauri 外安装兼容 `invoke` / `listen` 的 WebSocket adapter |
| 键盘注册表 | `frontend/lib/keyboard/registry.ts` | `attachShortcuts` |
| i18n | `frontend/lib/i18n/index.svelte.ts` + `locales/{en,zh}.ts` | `t('key')` |
| 设计令牌 | `frontend/styles/global.css` | `--bg --accent --raised --pressed` |

### 后端

| 概念 | 在哪 | 怎么验证 |
|---|---|---|
| Tauri command wrappers | `app/src/tauri/commands/*.rs` | 转交 `core/src/commands/` 共享逻辑 |
| Command 注册总表 | `app/src/tauri/mod.rs` 的 `generate_handler!` | 见 R3 |
| Headless command dispatcher | `app/src/server/mod.rs` | 见 R3；只暴露明确支持的同名 command |
| 全局运行态 | `core/src/state.rs` `AppState` | `rg 'AppState' core/src app/src` |
| Session lifecycle / ownership | `core/src/commands/lifecycle.rs` | reserve → activate → owner-scoped close/reconcile，见 R11 |
| 跨宿主事件抽象 | `core/src/emitter.rs` `Host` | 应用注入事件 sink 与可选窗口能力，不依赖 Tauri |
| SSH 客户端 | `core/src/ssh/client.rs` | russh wrapper |
| SSH 认证 / 算法策略 | `core/src/ssh/auth.rs` + `ssh/algorithms.rs` | password/key/agent/kbd-interactive + per-profile algorithms |
| Telnet | `core/src/terminal/telnet.rs` + `commands/telnet.rs` | 跨平台 TCP transport + profile CRUD |
| 串口 | `core/src/terminal/serial.rs` + `commands/serial.rs` | desktop-only serialport transport |
| Docker / K8s 动态发现 | `core/src/commands/discovery.rs` | source 持久化；发现结果临时；connector PTY 见 `commands/pty.rs` |
| SFTP | `core/src/ssh/sftp.rs` | russh-sftp wrapper |
| 端口转发 | `core/src/ssh/forward.rs` | local/remote/dynamic |
| PTY | `core/src/terminal/pty.rs`；入口由 `terminal/mod.rs` 的系统 cfg 选择 | 桌面 portable-pty；OHOS 按运行时能力开放 |
| 数据库 | `core/src/db/`，rusqlite bundled | 看 `db/schema.rs` |
| 数据迁移 | `core/src/migration/` + `db/schema.rs` | GUI / CLI 启动都必须走幂等 migration |
| Secret 抽象 | `core/src/secret/`，HybridStore + master-key backend + 加密 DB | 见 R5 |
| 错误类型 | `core/src/error.rs` `AppError` | thiserror 派生 |
| GitHub 同步 | `core/src/sync/github.rs` | `commands/sync.rs` 入口 |
| WebDAV 同步 | `core/src/sync/webdav.rs` | `commands/sync.rs` + CLI `rssh config webdav {set,push,pull}` |

### Tab ID 形态

| Tab type | 格式 | 出处 |
|---|---|---|
| `home` | 字面量 `"home"`，唯一固定，不可关闭 | `app.svelte.ts` 初始 `_tabs` |
| `ssh` / `local` / `serial` / `telnet` / `docker_exec` / `kubectl_exec` / `edit` | `"<type>:<uuid>"` | `crypto.randomUUID()` 调用点 |
| `forward` | `"fwd:<forward_id>:<timestamp>"` | `HomeScreen` / `osc/handler.ts` |

```bash
rg 'crypto\.randomUUID' frontend/lib   # 看 tab id 怎么造
```

### 构建命令

```bash
npm test                                                        # Vitest（含 ohos/tests）；CI gate
npm run build                                                   # Vite production build
cargo fmt --all --check
cargo test --workspace --all-features                           # Rust tests；先 npm run build
cargo check -p rssh-app --features tauri                        # GUI/lib
cargo check -p rssh-cli
cargo check -p rssh-app --features server --bin rssh-server
npm run tauri -- dev                                            # 本地 GUI
./scripts/build-mac.sh                                          # macOS aarch64 .dmg
./scripts/build-android.sh                                      # universal + ARM64 APK（需 Android SDK/NDK）
./scripts/build-ohos.sh --unsigned                              # 手机 / 电脑 HAP 与 App Pack
node --test ohos/scripts/*.test.mjs                             # OHOS 构建流程回归
# 固定上游源码准备后：
node --test ohos/framework/tests/*.test.mjs                     # OHOS 框架契约
```

当前没有 frontend lint script；有 Vitest、Rust unit/integration tests 与 Node 构建流程测试。`npm test` 包含 `ohos/tests/`，但不覆盖 `.mjs` Node 测试；修改打包流程需单独运行后者。UI、真实 SSH/Telnet/serial、Docker/K8s 等环境行为仍要按改动范围手测，不能拿编译替代。

---

## 坑（Pitfalls）— 非显然，会咬人

### P1. 启动 reconcile 与资源创建的顺序

主窗口必须先完成 owner-scoped `reconcile_sessions(activeIds=[])`，再放开 `resourcePanesAllowed` 让 Terminal / Forward / SFTP 创建后端资源；否则同一 owner 的新 session 会被启动清理误判成孤儿。Clone 与 AI handoff 窗口跳过这套主窗口初始化。后端现在按 `SessionOwner` 隔离，**不要再沿用“空列表会杀掉其他窗口所有 session”的旧解释**；真正不能破坏的是 owner 边界和 reconcile-before-create barrier。

```bash
rg '__rssh_clone|__rssh_ai_handoff|resourcePanesAllowed|reconcile_sessions' frontend core/src app/src
```

### P2. Linux CLI shadow GUI

`/usr/local/bin/rssh`（CLI）会 shadow `/usr/bin/rssh`（GUI）。CLI 在无 subcommand + 有 `DISPLAY`/`WAYLAND_DISPLAY` 时主动 fork GUI 二进制。改 CLI 启动逻辑必须保留 `canonicalize` 自循环检测。

```bash
rg 'try_launch_gui|RSSH_APP' cli/src
```

### P3. Highlight 是 xterm decoration，原始字节不可改

`TerminalPane.svelte` 把后端字节原样写入 xterm；`terminal/highlight-decorations.ts` 在 xterm 已解析的 cell grid 上注册 decoration。旧版“往输出流注入 ANSI”的方案会破坏 OSC/CSI 和原始颜色，已经废弃，不能复活。改这块要同时验证宽字符、组合字符、scrollback、resize/reflow、alternate buffer 和规则热更新。

### P4. SSH prompt waiter 带 owner 与 nonce

Keyboard-interactive、私钥 passphrase、host-key TOFU 分别使用 `auth_waiters` / `passphrase_waiters` / `host_key_waiters`；值是 `OwnedWaiter { owner, nonce, sender }`，key 是 prompt ID。`ssh::prompt::prompt_oneshot` 负责注册、emit、RAII 清理，respond/cancel 与 lifecycle 按 owner/nonce 防止旧 attempt 接管新 waiter。新增 prompt 类型不要另写一套裸 oneshot map。

### P5. CLI 直接读写 DB，不经 Tauri command

改表结构 / 改 SecretStore key 命名时，**同时审 CLI 路径**。CLI 不经过 Tauri command，但应复用 `db` / `secret` / `sync::config` 等共享 domain helper；只改 command wrapper 不会自动覆盖 CLI。

```bash
rg 'db::|secret_store|sync::config' cli/src
```

### P6. 本地备份与远程 push 的 secret 过滤不同

本地 export 是全量备份；远程 `rssh config github push` / `rssh config webdav push` 与 GUI push 才按同步类别、分组及 secret opt-in 过滤。Credential 用 `save_to_remote`，Telnet 登录脚本用 `save_script_to_remote`。改同步逻辑必须统一走 `sync::config::build_payload`，不能让 GUI / CLI / GitHub / WebDAV 四条路径各写一份规则。

### P7. 布局断点不能决定输入或原生能力

`stores/layout.svelte.ts` 统一观察窗口宽度，App 根组件管理监听生命周期。侧栏保留窄/宽两份偏好；resize 只切换读取，不改持久化设置。终端辅助键盘（包括唤起系统键盘的按钮）只在窄屏显示；宽屏不显示辅助键盘或独立浮钮，不按 OS、触摸能力或系统键盘是否打开增加例外。硬件键盘与系统输入法的输入处理独立于断点。

### P8. Command 名是跨适配器 wire contract

前端 `invoke("name")`、Tauri `generate_handler!` 与 headless `server.rs` dispatcher 都按字符串对接。Rust 函数或 wire 字段改名会让某个入口运行时炸；要改就全局 grep，并补 desktop + ipc-shim/headless 覆盖。

```bash
rg 'invoke\("|generate_handler!|"[a-z_]+" =>' frontend/lib app/src/tauri/mod.rs app/src/server/mod.rs
```

---

## 偏好（Preferences）— 风格 / 流程

### Pr1. 命名

- Rust：snake_case 函数与字段，PascalCase 类型
- TypeScript：camelCase 函数与变量，PascalCase 类型与组件
- State getter：动词短语 `tabs()` `activeTab()` `settingsActive()`，不带 `get` 前缀
- 错误：后端给稳定 code + params，前端按当前 locale 渲染中英文；不要在 Rust 里硬编码单一语言的用户文案

### Pr2. 错误处理

- Rust：`AppError` + `AppResult<T>`；业务错误用 `CodedMsg(code, params)`，序列化为 `__rssh_err__|{...}` wire format
- 前端：`catch (e) { toast.error(errMsg(e)) }`；不要直接把 coded wire 或裸 `String(e)` 展示给用户
- Headless：错误也要保持与 Tauri 相同的 wire shape，保证同一个 `errMsg()` 可本地化
- **不要**静默吞错（`.catch(() => {})`）除非确认是清理路径

### Pr3. CSS

- 用 `frontend/styles/global.css` 的变量与现有 `.neu-*` / `.btn*` 类。**不要**自己挑十六进制色——主题切换会破
- 图标自己画，别用emoji

### Pr4. 改动节奏

- 渐进式 > 大爆炸
- 一个 PR 一件事，不要顺手重构无关代码
- UI 改动跑 `npm run tauri dev` 实际点一遍，类型通过 ≠ 功能正确
- 所有分支禁止 force push；需要更新 PR 时追加正常 commit + 普通 `git push`
- 新分支先看现有命名，沿用 `feat/`、`fix/`、`chore/`、`perf/`、`refactor/`、`doc/` 前缀

### Pr5. 提交前自查

1. `npm test` + `npm run build` 通过
2. `cargo fmt --all --check` + `cargo test --workspace --all-features` 通过；按改动入口补 CLI 或 server 的 feature check（见“构建命令”）
3. 改了 command？检查 `lib.rs` handler；支持 headless 时同步 `server.rs` dispatcher（R3）
4. 改了事件名？前后端同步 grep `<domain>:`（R1）
5. 改了 schema？审 migration、sync import/export、CLI 共享路径（P5/P6）
6. 改了 UI？跑 dev 实际点；涉及 JetBrains/headless 时也走 `idea-plugin/scripts/dev-browser.mjs`

---

## 不要做的事（速查）

- 创建分析/计划文档（R6）
- 起新 IPC 通道（R3 / R2）
- 在组件里建跨页全局状态（R8）
- 把 secret 写 DB 明文（R5）
- 绕过 lifecycle registry 直接注册 transport handle（R11）
- 把动态发现结果保存成 Profile 或其他持久数据（R12）
- 对任何分支 force push（Pr4）
- 用 `--no-verify` 跳 hook
- 让 tab 根容器尺寸不明确或出现双层滚动（R4）
- 复制本文行号字面量进代码或 PR 描述（开头免责声明）
- 只为单一入口写功能而没声明其他入口的处理（R10）
- 改历史迁移块里的建表 / ALTER / 旧 seed SQL（R14）
