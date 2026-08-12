# 重构：每个工具独立的领域数据结构 + 共享执行信封

## 背景

现状：8 个工具蹭同一套 `CommandProposed`（10 字段）+ `command_proposed` 事件 + `CommandConfirmDialog`。
问题：`CommandProposed` 把**领域字段**（cmd/explain/side_effect/diff）、**执行字段**（full_cmd/sentinel/timeout_s）、**渲染**全揉在一起；`CommandConfirmDialog` 里 `isPatch`/`isAckOnly` 按 kind 分叉就是症状。patch 的命令字段是执行泄漏——它的领域是"改文件"不是"跑命令"。

## 目标分层

| 层 | 职责 | 归属 |
|---|---|---|
| **领域** | 每工具独立 Proposal/Result，只带自己需要的字段 | 10 个 |
| **执行信封** | `PtyExecution { full_cmd, sentinel, timeout_s }`，走 PTY 的工具共享，附加在 Proposal 上 | 1 个 |
| **基础设施** | reject(`command_rejected`) / ack(`ai_command_result`) / commandApprovals registry / executeCommand / commandExecutionStatus | 共享，不动 |

卡片只读领域字段；executeCommand（已独立，只认 id/full_cmd/sentinel/timeout_s）读执行信封。

## 阶段

### 阶段 1：download_file 独立线（tracer bullet）
**目标**: download_file 从 command 线彻底剥离，独立 Proposal/Result + 事件 + 卡片 + ChatItem kind。
**为什么先做它**: 非 PTY（走后端 SFTP），不涉及 PtyExecution 信封，最低风险，验证"独立线"完整模式。
**交付**:
- 后端: `DownloadProposal/Result` struct, `download_proposed/completed` 事件, `AuditKind::DownloadCompleted`, `handle_download_file` 改 emit
- 前端: `DownloadProposal/Result` types, `ChatItem.download`, store listener, `DownloadConfirmCard`, ChatPanel 路由, timeline restore/mutation, command-approval 映射, i18n
- `CommandKind` 移除 `download_file`
- resume 兼容: restoreTimeline 把旧 `command/download_file` 记录安全降级（丢弃或转 note），不崩
**成功标准**: cargo test + vitest + svelte-check 全绿；旧对话恢复不崩；download 全流程（审批→SFTP→卡片翻 done）手动可走
**状态**: 进行中

### 阶段 2：analyze_locally 独立线
**目标**: 同阶段 1 模式，analyze_locally 剥离。
**交付**: `AnalyzeProposal/Result`, `analyze_proposed/completed` 事件, `AnalyzeConfirmCard`, `CommandKind` 移除 `analyze_locally`，resume 兼容。
**成功标准**: 同阶段 1。
**状态**: 未开始

### 阶段 3：PtyExecution 信封 + patch×4 独立领域线（最复杂）
**目标**:
- 抽 `PtyExecution { full_cmd, sentinel, timeout_s }` 信封；`executeCommand` 入参从 `CommandProposed` 改 `PtyExecution`
- patch×4 各自独立领域 Proposal（path/find/replace/count/diff/overwrite_warning + execution 信封）
- patch×4 卡片各自独立（PatchCp/Modify/Diff/Mv Card），共享 PTY 执行器
**交付**: 4 个 patch Proposal/Result, patch 事件, 4 个卡片（或一个 PatchCard 按 step union）, ChatPanel 路由, `CommandKind` 移除 patch_*
**风险**: patch 是 4 步编排 + diff 在 step3→step4 传递 + 走 PTY。最高风险阶段。
**成功标准**: patch 4 步全流程可走；diff 正确在 mv 卡片显示；resume 不崩。
**状态**: 未开始

### 阶段 4：match_file + run_command 收尾
**目标**: match_file 独立（pattern + execution）；run_command 留作 `CommandProposed` 的最终形态（cmd/explain/side_effect + execution），或一并改名 `RunCommandProposal`。`CommandProposed` / `CommandKind` 在此阶段退役或只剩 run_command。
**成功标准**: 同上。
**状态**: 未开始

### 阶段 5：清理 + 全量回归
**目标**: 删所有死字段/死分支/死事件；`cargo test` + `vitest` + `svelte-check` 全绿；resume 回归（各时代 timeline blob）；手动验证每个工具卡片。
**状态**: 未开始

## 贯穿原则
- 每阶段一个 commit，可编译可测，不破坏 resume
- TDD：后端 handler test + 前端 timeline test 先行
- 一次只做一个阶段
