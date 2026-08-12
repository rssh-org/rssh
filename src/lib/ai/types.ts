/**
 * AI 排障模块类型，镜像 Rust 侧 src-tauri/src/ai/。
 */

export interface SkillRecord {
  id: string;
  name: string;
  description: string;
  content: string;
  builtin: boolean;
}

/**
 * 脱敏规则。默认规则首次运行 seed 进 DB，之后与用户规则一视同仁（无 builtin 字段）。
 * 规则变更只对新会话生效。
 */
export interface RedactRuleRecord {
  id: string;
  /** 正则源串 */
  pattern: string;
  /** 命中后替换成的占位符 */
  replacement: string;
}

/**
 * 命令黑名单的一个分类（后端按 5 类分组返回，顺序稳定）。
 * `category` 是稳定 key（destructive / write_verb / interpreter / deferred_exec /
 * forwarder），前端用它查 i18n 标签。整类编辑：保存即整类替换。
 * 空 commands = 该类被用户放行。改动只对新会话生效。
 */
export interface CategoryGroup {
  category: string;
  commands: string[];
}

export type LlmProvider = "anthropic" | "openai" | "deepseek" | "glm";

export interface AiSettings {
  provider: LlmProvider;
  model: string;
  endpoint: string | null;
  has_api_key: boolean;
  /** 危险模式总闸。off 时下面 8 个 auto_* 视同 false（持久化保留，方便切回时复原）。 */
  danger_mode: boolean;
  /** per-tool 自动批准。仅当 danger_mode=true 时生效；UI 上 danger 关时整组禁用。 */
  auto_run_command: boolean;
  auto_match_file: boolean;
  auto_download_file: boolean;
  auto_analyze_locally: boolean;
  auto_patch_cp: boolean;
  auto_patch_modify: boolean;
  auto_patch_diff: boolean;
  auto_patch_mv: boolean;
  auto_web_search: boolean;
  auto_web_fetch: boolean;
  /** 远端 shell 自动探测：off 时远端假设 POSIX；on 时 AI panel 打开时发探针。默认 off。 */
  auto_detect_remote_shell: boolean;
}

/** AI 工具卡片 kind —— 后端 emit command_proposed 时打的 tag；前端按它查 auto_* 设置。
 *  patch×4 / match_file 已迁到独立 ChatItem（见 PatchStep / MatchProposal），不在此列；
 *  CommandProposed 现在只服务 run_command。 */
export type CommandKind = "run_command";

/** web_search / web_fetch — dedicated proposal/result stream, independent of
 *  the command card (no full_cmd/sentinel/explain/side_effect). */
export type WebToolKind = "web_search" | "web_fetch";

export interface WebToolProposal {
  id: string;
  kind: WebToolKind;
  /** Redacted query (web_search) or URL (web_fetch). */
  target: string;
}

export interface WebToolResult {
  id: string;
  ok: boolean;
  summary: string;
  duration_ms: number;
}

/** download_file — SFTP pull of a remote artifact to local. Independent
 *  proposal/result stream; ack-only (approve just acks, the backend does the
 *  SFTP transfer itself — no PTY execution). */
export interface DownloadProposal {
  id: string;
  remote_path: string;
  max_mb: number;
  dest_dir: string;
}

export interface DownloadResult {
  id: string;
  ok: boolean;
  local_path?: string;
  bytes?: number;
  summary: string;
  duration_ms: number;
}

/** analyze_locally — spawn a new window with an independent AI session to
 *  analyze a local artifact. Independent proposal/result stream; ack-only
 *  (approve just acks, the backend spawns the window itself, no PTY). */
export interface AnalyzeProposal {
  id: string;
  local_path: string;
  task: string;
}

export interface AnalyzeResult {
  id: string;
  ok: boolean;
  summary: string;
  duration_ms: number;
}

/** patch_file — 4-step staged edit (cp → modify → diff → mv), each step a PTY
 *  execution sharing the PtyExecution envelope. Independent proposal/result
 *  stream (patch_proposed / patch_completed); reuses executeCommand + the
 *  shared reject (command_rejected) / ack (ai_command_result) channels.
 *
 *  `step` discriminates which fields are meaningful; optionals are absent on
 *  the steps that don't use them. The result is a PTY CommandResult (exit code
 *  + output), same shape as run_command. */
export type PatchStep = "cp" | "modify" | "diff" | "mv";

export interface PatchProposal {
  id: string;
  step: PatchStep;
  /** Shell command to run (shown for trust). */
  cmd: string;
  /** Target file being patched. */
  path: string;
  /** Staging tmp path (cp / modify / diff / mv). */
  tmp_path?: string;
  /** modify only. */
  find?: string;
  replace?: string;
  expected_count?: number;
  /** mv only: the unified diff to show on the apply card. */
  diff?: string;
  execution: PtyExecution;
}

/** match_file — read-only search of `find` in a remote file. Independent
 *  proposal/result stream (match_proposed / match_completed); reuses
 *  executeCommand + the shared reject/ack channels. Result is the PTY
 *  CommandResult (the raw search output). */
export interface MatchProposal {
  id: string;
  /** Shell command to run (shown for trust). */
  cmd: string;
  /** File to search. */
  path: string;
  /** Search string. */
  find: string;
  /** Context chars before/after each match (clamped by the backend). */
  before: number;
  after: number;
  execution: PtyExecution;
}

export interface ModelInfo {
  id: string;
  display_name: string | null;
}

export interface AiSessionInfo {
  /** Tab 内的会话身份；切 Tab / SSH 重连不变，显式关闭 AI 面板时结束。 */
  tab_id: string;
  /** 后端 actor 的不可复用身份；同一 tab 关闭重开会得到新值。 */
  instance_id: string;
  /** 当前绑定的 SSH/PTY session_id。重连时由 rebindTarget 更新。 */
  target_id: string;
  skill: string;
  model: string;
  provider: LlmProvider;
  /** ai_conversations 行 id —— timeline 自动保存按它写库；resume 沿用旧 id。 */
  conversation_id: string;
}

/** 历史对话列表项（无 blob，给 picker 用）。时间戳为 epoch 毫秒。 */
export interface ConversationMeta {
  id: string;
  title: string;
  created_at: number;
  updated_at: number;
}

/** 远端 shell 三家族（lowercase wire format）。Rust 端 ShellKind 还有
 *  serial/telnet 两个 raw-device 变体，但它们只在后端由 AiTarget 推出、
 *  从不走这条 probe 通道 —— 这个类型只覆盖 SSH 探测能产出的家族。 */
export type ShellKind = "posix" | "cmd" | "powershell";

/**
 * Which transport an AI session targets — mirrors Rust `AiTarget` kinds
 * (lowercase wire format). Single source of truth so widening the set (e.g.
 * adding `serial`) lights up every switch via the compiler.
 */
export type AiTargetKind = "ssh" | "local" | "serial" | "telnet";

/**
 * Raw-device targets run without a shell: no sentinel, no meaningful exit
 * code, user-driven completion ("submit output"). Mirrors Rust
 * `ShellKind::is_raw_device` — one predicate so the execution paths and the
 * confirm-dialog affordances can't drift apart.
 */
export function isRawDeviceKind(kind: AiTargetKind): boolean {
  return kind === "serial" || kind === "telnet";
}

/** Cumulative token spend for one AI session (actor lifetime). */
export interface TokenUsage {
  tokens_in: number;
  tokens_out: number;
}

/** 一条对话消息（前端展示用） */
export type ChatItem =
  | { kind: "user"; client_id?: string; client_seq?: number; text: string; at: number }
  | { kind: "assistant"; id: string; text: string; at: number; streaming: boolean; cancelled?: boolean }
  | { kind: "command"; cmd: CommandProposed; at: number; result?: CommandResult; rejected?: { reason: string } }
  | { kind: "web_tool"; proposal: WebToolProposal; at: number; result?: WebToolResult; rejected?: { reason: string } }
  | { kind: "download"; proposal: DownloadProposal; at: number; result?: DownloadResult; rejected?: { reason: string } }
  | { kind: "analyze"; proposal: AnalyzeProposal; at: number; result?: AnalyzeResult; rejected?: { reason: string } }
  | { kind: "patch"; proposal: PatchProposal; at: number; result?: CommandResult; rejected?: { reason: string } }
  | { kind: "match"; proposal: MatchProposal; at: number; result?: CommandResult; rejected?: { reason: string } }
  | { kind: "error"; text: string; at: number }
  | { kind: "note"; text: string; at: number };

/** PTY execution envelope — the "how to run it" half of a proposal, shared by
 *  every tool whose approval pastes a command into the terminal (run_command /
 *  match_file / patch×4). Split out from each tool's domain fields so
 *  `executeCommand` takes just (cardId, execution) and is reused across proposal
 *  types instead of depending on any one of them. */
export interface PtyExecution {
  /** 实际要粘贴到终端的命令（含 sentinel + exit code 回显），由后端拼装。 */
  full_cmd: string;
  /** 用于在 PTY 输出流里识别命令完成的随机字符串。 */
  sentinel: string;
  timeout_s: number;
}

export interface CommandProposed {
  id: string;
  /** Compatibility wire alias. New backends set this to the per-card `id`; the
   * provider's tool-call id remains backend-internal. */
  tool_call_id: string;
  cmd: string;
  explain: string;
  side_effect: string;
  execution: PtyExecution;
  /**
   * 工具卡片类型 —— 前端按 kind 查 settings.auto_<kind> 决定是否自动批准。
   * 历史回放（旧 audit log 重渲染）可能没有 kind，按未知处理走人审。
   */
  kind?: CommandKind;
  /**
   * patch_file 第 4 张 mv 卡片携带的 diff 文本（来自第 3 张 diff 命令的输出）——
   * 让用户审批 mv 时直接在卡片上看到 diff，不用回滚翻第 3 张的 result 区域。
   * 其他卡片不带（undefined）。
   */
  diff?: string;
}

export interface CommandResult {
  id: string;
  exit_code: number;
  timed_out: boolean;
  /** 用户在执行中点了"提前终止"。 */
  early_terminated?: boolean;
  duration_ms: number;
  output: string;
  original_bytes: number;
  truncated_bytes: number;
}

/** 审计日志（来自后端 ai_audit_get） */
export interface AuditLog {
  entries: AuditEntry[];
}
export interface AuditEntry {
  at: string; // ISO 8601
  kind: AuditKind;
}
export type AuditKind =
  | { type: "session_started"; skill: string; target: string }
  | { type: "session_ended" }
  | { type: "llm_request"; model: string; redacted_payload: string }
  | { type: "llm_response"; text: string; tokens_in: number | null; tokens_out: number | null }
  | { type: "command_proposed"; id: string; cmd: string; explain: string; side_effect: string }
  | { type: "command_rejected"; id: string; reason: string }
  | { type: "command_blocked"; cmd: string; reason: string }
  | { type: "command_executed"; id: string; exit_code: number; output_redacted: string; original_bytes: number; truncated_bytes: number; duration_ms: number }
  | { type: "download_proposed"; id: string; remote_path: string; max_mb: number }
  | { type: "download_completed"; id: string; local_path: string; bytes: number }
  | { type: "analyze_proposed"; id: string; local_path: string; task: string }
  | { type: "skill_loaded"; id: string; name: string }
  | { type: "web_search_completed"; query: string; provider: string; response_bytes: number; duration_ms: number }
  | { type: "web_fetch_completed"; requested_url: string; final_url: string; source_bytes: number; truncated: boolean }
  | { type: "context_rolled_back"; user_message_index: number; dropped_messages: number }
  | { type: "note"; message: string }
  | { type: "error"; message: string };

export type AiPanelPosition = "left" | "right";
