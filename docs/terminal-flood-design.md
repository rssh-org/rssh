# 终端洪水输出治理：积压指示器 + Ctrl+C 泄压 + WebGL 渲染

> 状态：设计已确认（对话中逐节通过），待 Linus 最终签字。最后更新：2026-08-14。

## 1. 问题与诊断

大输出（如 `grep` 翻江倒海）时 Ctrl+C "打不断"；从另一会话 `kill -9` 后本端仍在持续输出。

根因不是 Ctrl+C 没送到，而是**整条输出链路没有任何一处背压，所有积压都堆在最慢一环（DOM 渲染器）前面，且每层队列无界**：

```
远端 grep → sshd → TCP → russh channel.wait()
  → session_task: app.emit("ssh:data:<sid>", Vec<u8>)      src-tauri/src/ssh/client.rs:967
  → webview 事件循环（主线程忙时事件无界排队）
  → terminal.write(raw)                                    src/lib/components/TerminalPane.svelte:932
  → xterm WriteBuffer（无界；>50MB 直接 throw）             node_modules/.../WriteBuffer.ts:104
  → 每轮事件循环最多解析 12ms                               WriteBuffer.ts:28
  → DomRenderer 重绘（xterm 6 默认，最慢的渲染器）
```

三个事实：

1. `\x03` 的发送路径（`sendText → invoke → tokio::select!`）不被输出淹没，几乎立刻到达远端，生产者立刻死。用户看到"打不断"，是消费者落后几十 MB 在还债。`kill -9` 后仍输出同理：各层队列已积压的数据必须全部流过最慢一环。
2. xterm 6 已删除 canvas 渲染器，官方推荐只有 DomRenderer 或 WebGL（`@xterm/addon-webgl`）。rssh 目前用默认 DomRenderer。
3. Tauri 事件把 `Vec<u8>` 序列化成 JSON 数字数组（每字节一个 number，约 4–6 倍膨胀），32KB 的 SSH 块 ≈ 3.2 万元素的数组要 `JSON.parse`。主线程被"解析 + 渲染"吃满，keydown 排队尾，连发出 Ctrl+C 本身都迟钝。

潜伏崩溃：xterm 的 `write()` 在 pending > 50MB 时直接 throw（`WriteBuffer.ts:104`，官方注释承认 >500KB 即无响应）。大 grep 可以踩到。

## 2. 方案取舍

| 方案 | 结论 |
| --- | --- |
| A. 需求驱动的喂数队列（output-feeder） | ✅ 采用。积压握在自己手里，xterm 内部 ≤1 chunk；记账精确、可丢弃、结构性消灭 50MB throw |
| B. 直通 + 挖 xterm 私有 `_writeBuffer` 清空 | 弃。私有 API，升级即碎 |
| C. 只记账不持有 | 弃。指示器可做，但"丢弃"无从谈起 |

**与已删除的 write-batcher（commit 0caaf12，#213）的本质区别**：batcher 是定时器——每个写先等 8ms/64KB 合并，vim/less 天天付延迟税。feeder 无定时器、无合并：队列空时数据**直接透传**（vim/less 路径零开销）；只有积压时才按 xterm 的消化节奏（上一个 chunk 的 write 回调）喂下一个。batcher 的三类复杂度（timer state、dispose 排序、flush 调用点）一个都不回来。

## 3. 设计

### 3.1 `src/lib/terminal/output-feeder.ts`（新模块）

```ts
createOutputFeeder(opts: {
  write: (data: Uint8Array | string, cb: () => void) => void;  // 即 terminal.write
  maxPendingBytes: number;
}): {
  push(raw: Uint8Array | string): void;  // 数据事件入口
  pendingBytes(): number;                // 队列字节 + 在途 chunk 字节
  dropPending(): void;                   // 清空队列（在途 chunk 照常解析完，≤1 chunk）
  dispose(): void;
}
```

**不变量**：xterm 内部未解析数据 ≤ 1 chunk；`pendingBytes()` = 队列字节 + 在途 chunk 字节。

全部变更点逐一核对：

- `push`（队列空）：直接 `write(chunk, cb)`，cb 触发时若队列非空则喂下一个。零额外延迟。
- `push`（积压中）：入队。只在 cb 链上续喂。
- drain 回调：在途 chunk 清零 + 队列头出队成为新的在途 chunk。
- `dropPending`：清空队列；在途 chunk 已交给 xterm，让它解析完（≤32KB，一帧）。
- 内存上限：队列字节 > `maxPendingBytes` 时丢最旧整个 chunk（洪水里丢旧数据最多错一行渲染）。常量进 `src/lib/terminal/limits.ts`：桌面 128MB / 移动 32MB。
- `dispose`：清队列，不再续喂。

接入点（全部数据事件写点）：`writeRawOutput`（ssh/pty，TerminalPane.svelte:252）、stream 分支的 `terminal.write(streamNormalizeOut(...))`（:927）、hex 模式的 `terminal.write(bytesToHex(raw))`（:926）。合成 UI 写入（prompt、断线横幅）**绕过** feeder，直达 terminal。

断线（`announceDisconnected`）时调用 `dropPending()`：事件已被 `acceptsSessionEvent` 过滤，但队列里的旧洪水不该在断线横幅后面继续流。

### 3.2 Ctrl+C 泄压阀

触发条件：任一发送路径发现 data 含 `\x03` 且 `feeder.pendingBytes() > 256KB`。

动作两步，缺一不可：

1. `dropPending()`——清掉手里的大头；
2. 进入**静默丢弃**模式：后续 `push` 直接丢，直到 **150ms 无新数据**（或连接关闭/断线）。必须如此——webview 事件队列里还压着旧洪水事件，drop 完它们会立刻重新灌满。gap 之后的数据（shell 提示符）正常渲染。

`\x03` 照常发送杀生产者。三个发送点共用一个 helper（如 `maybeReleaseBacklog(data)`）：

- `wireSessionInput` 的 `terminal.onData`（TerminalPane.svelte:992）
- `writePty`（:1762，AI 命令路径）
- stream 发送路径（:642/:653，串口/telnet slow-send）

静默门风险：仅在一次"用户显式 Ctrl+C + 大积压"后激活，首个 gap 即退出。最坏情形多丢一段洪水尾部——本来就是用户要扔的东西。

### 3.3 右上角积压指示器

- `pendingBytes > 64KB` 自动浮现，回落后淡出；显示人读格式（`1.2 MB`）+ 副文案「Ctrl+C 跳过积压」。
- 更新走独立的 rAF 节流（指示器自持一个 rAF 合并，不复用 paint scheduler——后者受 block tracker 的 `shouldPaint` 条件约束，语义不同），**不**每个数据事件都打 Svelte 反应性。
- 文案进 `src/lib/i18n/locales/{zh,en}.ts`。
- 定位在终端区右上角（搜索框下方），absolute，不抢焦点。

### 3.4 WebGL 渲染

- 依赖 `@xterm/addon-webgl@0.19.0`（xterm 6.0 配对稳定版；0.20 beta 锁 6.1 内核，不用）。
- `terminal.open()` 后 `loadAddon(new WebglAddon())`，包 try/catch：失败（老 GPU / 远程桌面 / WebView 无 WebGL2）回退 DomRenderer 并记日志。
- `addon.onContextLoss(() => addon.dispose())`——dispose 后 xterm 自动回退 DomRenderer。
- 与 image addon（SIXEL/IIP）、HighlightDecorator、search 兼容（VS Code 同款组合）。无新增配置项。

### 3.5 常量汇总（limits.ts，均可调）

| 常量 | 值 | 用途 |
| --- | --- | --- |
| `BACKLOG_INDICATOR_BYTES` | 64 KB | 指示器浮现阈值 |
| `BACKLOG_DROP_TRIGGER_BYTES` | 256 KB | Ctrl+C 触发丢弃的积压阈值 |
| `BACKLOG_QUIESCENCE_MS` | 150 ms | 静默丢弃的 gap 判定 |
| `BACKLOG_MAX_PENDING_BYTES` | 128 MB 桌面 / 32 MB 移动 | 队列内存上限（丢最旧） |

## 4. 明确不做

- **Rust 侧背压**（pause/resume + SSH 窗口收缩让 grep 阻塞在 write()）：指示器落地后拿真实数据说话，真需要再做。
- **`Vec<u8>` → JSON number[] 膨胀改 base64/binary**：约 10 倍削减主线程解析成本，但要动 4 个传输的 Rust 发射端 + 前端监听端 + headless ws server，独立 PR。
- 定时器式 batch/flush：#213 已删，不复活。

## 5. 测试

- feeder 单测（vitest + 真 Terminal，仿 `command-blocks.test.ts` 模式）：空闲直通零延迟（无 setTimeout 介入）、积压时按回调节奏续喂、`dropPending` 清空且在途 chunk 不损、超限丢最旧、dispose 后不再喂。
- 静默门单测：模拟"洪水 + \x03 + gap + 新数据"事件序列，断言 gap 后数据恢复渲染、gap 前的迟到洪水被丢。
- 断线路径：`announceDisconnected` 后队列清空、横幅后无残余输出。
- WebGL：CI 无 GPU，手动 QA（桌面 + Android + iOS WebView），验证回退路径有日志、SIXEL 图与关键词高亮正常。

## 6. 风险

| 风险 | 缓解 |
| --- | --- |
| feeder 丢回调 → 终端停摆 | 回调由 `WriteBuffer._innerWrite` 保证触发；`dispose` 清空兜底；单测覆盖续喂链 |
| 静默门误吃新数据 | 仅在显式 Ctrl+C + 大积压后激活；150ms gap 即退出；丢的只是洪水尾部 |
| WebGL 在部分环境不可用 | try/catch 回退 DomRenderer，onContextLoss 同样回退，全程有日志 |
| 与已删 batcher 混淆 | 实现里无任何 timer/flush 概念；评审时按此对照 |
