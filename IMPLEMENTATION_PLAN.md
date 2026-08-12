# 重构：每个工具独立的领域数据结构 + 共享执行信封

## 背景

现状（阶段 1+2 完成后）：download/analyze/web 已独立线；PTY 工具（run/match/patch×4）仍蹭 `CommandProposed` + `command_proposed` + `CommandConfirmDialog` + `executeCommand`。
问题：`CommandProposed` 把领域、执行、渲染揉一起；`CommandConfirmDialog` 的 `isPatch` 分叉是症状。patch 的命令字段是执行泄漏——领域是"改文件"。

## 目标分层

| 层 | 职责 | 归属 |
|---|---|---|
| **领域** | 每工具独立 Proposal/Result，只带自己需要的字段 | 每工具一个 |
| **执行信封** | `PtyExecution { full_cmd, sentinel, timeout_s }`，走 PTY 的工具共享，附在 Proposal 上 | 1 个 |
| **基础设施** | reject(`command_rejected`) / ack(`ai_command_result`) / commandApprovals / executeCommand / commandExecutionStatus | 共享，不动 |

## 已完成

### 阶段 1：download_file 独立线 ✅ (commit 7b8ece3)
### 阶段 2：analyze_locally 独立线 ✅ (commit 9c95845)
两个 ack-only 工具（后端自己执行，不碰 PTY）。各自独立 Proposal/Result + 事件（`download_proposed/completed`、`analyze_proposed/completed`）+ 独立卡片（DownloadConfirmCard / AnalyzeConfirmCard）。reject/ack 复用。旧 command 记录在 `restoreTimeline` 丢弃。模式验证通过。

---

## 阶段 3：PtyExecution 信封 + patch×4 领域化（**最高风险，最复杂**）

### 起点（handoff 时的代码事实）
- `executeCommand`（store.svelte.ts:1346）**只读** `proposed.{id, full_cmd, sentinel, timeout_s}` —— 正好是 PtyExecution + id。改造点明确。
- `handle_patch_file`（file_ops.rs:760）：4 步 cp→modify→diff→mv，调 `run_file_op`（file_ops.rs:539），领域信息（path/find/replace/tmp_path）现在编码进 explain 文本，没结构化。
- `run_file_op`（file_ops.rs:539）：patch + match 共用的 PTY 执行核，emit command_proposed/completed。
- patch 的 `diff` 字段已在 step3→step4 传递（run_file_op 的 diff 参数，L572-574）。

### 3a：PtyExecution 信封（先做，纯重构，行为不变）
1. **types.ts**：定义 `PtyExecution { full_cmd, sentinel, timeout_s }`。`CommandProposed` 的 full_cmd/sentinel/timeout_s 重组进 `execution: PtyExecution` 字段。
2. **executeCommand**（store.svelte.ts:1346）：签名从 `(session, proposed: CommandProposed, ...)` 改 `(session, cardId, execution: PtyExecution, ...)`。它本来就只读那 4 个字段（1359/1508/1535/1537/1551），逻辑不动。
3. **调用方**：`CommandConfirmDialog.svelte` 的 approve（grep `executeCommand` 调用点）传 `{ id: cmd.id, execution: cmd.execution }`。
4. **后端**：`handle_run_command`（session.rs:1685）+ `run_file_op`（file_ops.rs:539）emit 的 command_proposed payload 把 full_cmd/sentinel/timeout_s 嵌进 `execution` 对象。
5. **验证**：PTY 命令全流程仍工作（run/match/patch）。cargo + vitest + 手动一条 run_command。

### 3b：patch×4 领域化
1. **后端 handle_patch_file**（file_ops.rs:760）：4 步的领域字段从 explain 文本提到结构化——
   - cp: `{ path, tmp_path, execution }`
   - modify: `{ path, find, replace, expected_count, execution }`
   - diff: `{ path, execution }`
   - mv: `{ path, diff, execution }`
   `run_file_op` 的 PTY 执行核保留，emit 改 `patch_proposed/completed`（领域 + execution 信封）。
2. **前端**：4 个 PatchProposal（或 1 个带 step）+ `ChatItem.patch` + `PatchConfirmCard`（从 CommandConfirmDialog 抽 `isPatch` 逻辑 + diff 框，L39/L294）+ patch 事件 listener + timeline + `CommandKind` 移除 patch_*。
3. **CommandConfirmDialog** 删 isPatch 分支（只剩 run/match）。

### 风险（红线）
1. **executeCommand 双粘**：重入 guard（`_commandExecutions` map，store.svelte.ts:1359-1368）必须保持。改入参时别破坏 guard —— 双粘 rm/reboot 是灾难。
2. **patch 编排**：cp→modify→diff→mv 顺序 + tmp_path + count 校验。领域化**只改 emit 字段，不改 4 步逻辑**。错了 = 用户文件损坏（不可逆，never break userspace 红线）。
3. **resume**：旧 `command/patch_*` 记录丢弃（timeline isRenderable command case 加 patch_* 迁移检查，仿 download_file/analyze_locally）。

### 测试
- timeline patch restore/mutation（平行 download/analyze 测试，timeline.test.ts）
- PTY 执行 e2e（仿 reference_ws_server_e2e_harness，或现有 command 测试）
- 手动：patch_file 4 步全流程 + diff 在 mv 卡片显示 + 断网 resume

### 阶段 3 必须前后端 + 测试一次性完成
前端听 patch_proposed、后端发 patch_proposed、executeCommand 改入参——三者任一半途，PTY 链断，编译不过。不能留中间态。

---

## 阶段 4：match_file + run_command 收尾
match_file 独立（pattern + execution）；run_command 留作 CommandProposed 最终形态或改名 RunCommandProposal。CommandConfirmDialog 退役/重构（删 isAckOnly 死分支，isAckOnly 已恒 false）。

## 阶段 5：清理 + 全量回归
删死字段/死分支；cargo + vitest + svelte-check 全绿；resume 回归；手动每个工具卡片。

## 贯穿原则
- 每阶段一个 commit，可编译可测，不破坏 resume（旧记录丢弃即可，不强迁）
- TDD
- reject/ack/commandApprovals/executeCommand 是共享基础设施，不动其语义
