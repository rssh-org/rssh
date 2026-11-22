/**
 * Background SFTP transfers — independent of any open SftpBrowser overlay.
 *
 * Each transfer opens its own sftp subsystem on a live SSH session, runs to
 * completion, then closes. Closing the browser does not affect anything here:
 * we never share its sftp_id.
 *
 * 进度事件遵守 AGENT.md R1：`sftp:progress:{transfer_id}` 三段式。每条 transfer
 * 起一个 listener、跑完即解绑——避免一个全局 listener 被全部 transfer 喂事件。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import * as app from "./app.svelte.ts";
import { toast } from "./toast.svelte.ts";
import { errCoded, errMsg, t } from "../i18n/index.svelte.ts";

export type TransferKind = "download" | "upload";
type TransferPhase = "verifying" | "transferring" | "publishing";
export type TransferStatus = "queued" | "running" | "done" | "failed" | "cancelled";

/// Global concurrency cap: at most N running transfers at a time; the rest
/// stay queued. Default 10 is a pragmatic sweet spot — comfortably within
/// SSH channel limits (typically 10-100) while keeping per-connection
/// bandwidth share reasonable. User-overridable in Shell settings, bounded
/// [MIN, MAX] —— 太低用户搬大量小文件慢得难受；太高撞 SSH 服务端 channel 限或
/// 把单条 SSH 连接带宽切成碎片。
const DEFAULT_MAX_CONCURRENT = 10;
const MIN_MAX_CONCURRENT = 1;
const MAX_MAX_CONCURRENT = 20;
const SETTING_KEY_MAX_CONCURRENT = "sftp_max_concurrent";

let _maxConcurrent = $state(DEFAULT_MAX_CONCURRENT);
/// 标记"本地已经显式 set 过"。loadMaxConcurrent 是 fire-and-forget，可能晚于
/// 用户在 Settings 里的保存才解出 DB 旧值——那次旧值不能反过来盖掉刚 save 的
/// 新值。set 之后置 true，后续 load 看到 true 就跳过覆盖。
let _setLocally = false;

export function maxConcurrent(): number { return _maxConcurrent; }
export function maxConcurrentBounds(): { min: number; max: number; def: number } {
  return { min: MIN_MAX_CONCURRENT, max: MAX_MAX_CONCURRENT, def: DEFAULT_MAX_CONCURRENT };
}

/** App 启动时调一次 —— 从 DB 拉持久化值覆盖默认。失败保留默认值，不阻塞。
 *  若期间用户已经手动 set 过（_setLocally==true），就别用 DB 旧值反向覆盖。 */
export async function loadMaxConcurrent(): Promise<void> {
  try {
    const v = await invoke<string | null>("get_setting", { key: SETTING_KEY_MAX_CONCURRENT });
    if (_setLocally) return;
    if (v) {
      const n = parseInt(v, 10);
      if (Number.isFinite(n) && n >= MIN_MAX_CONCURRENT && n <= MAX_MAX_CONCURRENT) {
        _maxConcurrent = n;
      }
    }
  } catch {
    // setting backend 没就绪等情况：留默认 10 即可
  }
}

/** ShellSettings 改值时调。clamp 到 [MIN, MAX]，先写 DB 成功再改 runtime
 *  —— DB 失败时 runtime 不动，避免重启后两者不一致。最后 kick 一下队列让
 *  调大后多出来的额度立刻消化掉 queued 项；调小不主动 cancel 正在跑的。 */
export async function setMaxConcurrent(n: number): Promise<void> {
  const clamped = Math.max(MIN_MAX_CONCURRENT, Math.min(MAX_MAX_CONCURRENT, Math.floor(n)));
  await invoke("set_setting", { key: SETTING_KEY_MAX_CONCURRENT, value: String(clamped) });
  _maxConcurrent = clamped;
  _setLocally = true;
  while (runningCount() < _maxConcurrent) {
    const before = runningCount();
    promoteNextQueued();
    if (runningCount() === before) break; // 没 queued 了
  }
}

interface TransferOrigin {
  sessionId: string;
  tabId?: string;
  profileId?: string;
}

interface TransferFiles extends TransferOrigin {
  localPath: string;
  remotePath: string;
}

export interface Transfer extends TransferFiles {
  id: string;
  kind: TransferKind;
  grantId?: number;
  attempt: number;
  cancelRequested?: boolean;
  total: number;
  transferred: number;
  status: TransferStatus;
  phase?: TransferPhase;
  partialPath?: string;
  backupPath?: string;
  error?: string;
  startedAt: number;
  finishedAt?: number;
}

let _list = $state<Transfer[]>([]);

interface ProgressPayload {
  phase?: TransferPhase;
  partialPath?: string;
  backupPath?: string;
  transferred: number;
  total: number;
}

async function attachProgressListener(id: string, attempt: number): Promise<UnlistenFn> {
  let firstProgress = true;
  return await listen<ProgressPayload>(`sftp:progress:${id}`, (ev) => {
    const t = _list.find((x) => x.id === id);
    if (t?.status !== "running" || t.attempt !== attempt) return;
    if (firstProgress) {
      firstProgress = false;
      // Independent IPC calls may deliver Cancel before the backend registers
      // this attempt. Its first progress event acknowledges registration.
      if (t.cancelRequested && ev.payload.phase !== "publishing") void sendCancel(t);
    }
    if (ev.payload.backupPath) t.backupPath = ev.payload.backupPath;
    if (ev.payload.partialPath) t.partialPath = ev.payload.partialPath;
    t.phase = ev.payload.phase ?? "transferring";
    t.transferred = ev.payload.transferred;
    t.total = ev.payload.total;
  });
}

export function list(): Transfer[] {
  return _list;
}

export function activeCount(): number {
  return _list.filter((t) => t.status === "running" || t.status === "queued").length;
}

/** Look up a transfer in the proxied store; needed so mutations trigger reactivity. */
function find(id: string): Transfer | undefined {
  return _list.find((x) => x.id === id);
}

function runningCount(): number {
  return _list.filter((t) => t.status === "running").length;
}

/// Promote the queued transfer matching `id` to running and kick it off.
/// Returns early when already running/cancelled/done/etc. When the capacity
/// is full, the entry remains queued and is picked up by the next
/// `promoteNextQueued()` scan in some other transfer's `finally` block.
function tryDispatch(id: string): void {
  if (runningCount() >= _maxConcurrent) return;
  const t = find(id);
  if (!t || t.status !== "queued") return;
  t.status = "running";
  t.startedAt = Date.now();
  void runTransfer(id);
}

/// Called from runTransfer's finally: scan for the oldest queued entry and
/// promote one. Iterates from the tail because `startDownload` unshifts new
/// transfers to the front, so the oldest queued entry lives at the end.
function promoteNextQueued(): void {
  if (runningCount() >= _maxConcurrent) return;
  for (let i = _list.length - 1; i >= 0; i--) {
    if (_list[i].status === "queued") {
      tryDispatch(_list[i].id);
      return;
    }
  }
}

// A queued first attempt stays bound to the selected SSH connection. On
// retry the backend compares the new session's authenticated peer and account
// with the transfer checkpoint before it permits any file access.
function sessionForAttempt(item: Transfer): string {
  if (item.attempt === 1 || !item.tabId) return item.sessionId;
  const current = app.sessionIdForTab(item.tabId);
  if (current) return current;
  if (item.profileId) {
    for (const tab of app.tabs()) {
      if (tab.type !== "ssh" || tab.meta?.profileId !== item.profileId) continue;
      const session = app.sessionIdForTab(tab.id);
      if (session) return session;
    }
  }
  throw new Error(t("sftp.reconnect"));
}

async function runTransfer(id: string): Promise<void> {
  const item = find(id);
  if (!item) return;
  let sftpId: string | undefined;
  let unlisten: UnlistenFn | undefined;
  try {
    unlisten = await attachProgressListener(id, item.attempt);
    if (!item.cancelRequested) {
      sftpId = await invoke<string>("sftp_connect_session", { sessionId: sessionForAttempt(item) });
    }
    if (item.cancelRequested) {
      item.status = "cancelled";
      return;
    }
    const command = item.kind === "download" ? "sftp_download_to" : "sftp_upload_from";
    const bytes = await invoke<number>(command, {
      sftpId,
      remotePath: item.remotePath,
      localPath: item.localPath,
      ...(item.grantId === undefined ? {} : { grantId: item.grantId }),
      transferId: id,
      originalSessionId: item.sessionId,
    });
    item.transferred = bytes;
    item.total = bytes;
    item.status = "done";
  } catch (error) {
    const code = errCoded(error)?.code;
    if (!code && item.phase === "publishing" && item.backupPath) {
      const path = item.kind === "download" ? item.localPath : item.remotePath;
      item.error = "__rssh_err__|" + JSON.stringify({
        code: "sftp_commit_uncertain", params: { path, backup: item.backupPath },
      });
    } else {
      item.error = error instanceof Error ? error.message : String(error);
    }
    if (code === "transfer_cancelled") item.status = "cancelled";
    else if (code === "sftp_backup_cleanup_failed") item.status = "done";
    else item.status = "failed";
  } finally {
    item.finishedAt = Date.now();
    unlisten?.();
    if (sftpId) void invoke("sftp_close", { sftpId }).catch((error) => {
      console.warn("[sftp] close transfer connection:", error);
    });
    promoteNextQueued();
  }
}

function enqueue(kind: TransferKind, files: TransferFiles, sizeHint: number, grantId?: number): string {
  const id = crypto.randomUUID();
  _list.unshift({
    id,
    kind,
    grantId,
    sessionId: files.sessionId,
    tabId: files.tabId,
    profileId: files.profileId,
    localPath: files.localPath,
    remotePath: files.remotePath,
    attempt: 1,
    total: sizeHint,
    transferred: 0,
    status: "queued",
    startedAt: Date.now(),
  });
  tryDispatch(id);
  return id;
}

export async function startDownload(args: TransferFiles & { sizeHint?: number; grantId?: number }): Promise<string> {
  return enqueue("download", args, args.sizeHint ?? 0, args.grantId);
}

export async function startUpload(args: TransferFiles): Promise<string> {
  return enqueue("upload", args, 0);
}

// These errors require inspecting the reported paths or starting a transfer
// with a different destination or connection.
const nonRetryableErrors = new Set([
  "sftp_publish_failed", "sftp_backup_failed", "sftp_replace_failed",
  "sftp_commit_uncertain", "sftp_backup_changed", "sftp_transfer_reconnect_required",
  "sftp_folder_operations_unsupported",
]);

export function canRetry(item: Transfer): boolean {
  if (item.status !== "failed") return item.status === "cancelled";
  const error = errCoded(item.error);
  if (error?.code === "sftp_provider_siblings_unavailable" && error.params?.action === "reselect") return false;
  return !nonRetryableErrors.has(error?.code ?? "");
}

export async function retry(id: string): Promise<void> {
  const t = find(id);
  if (!t || !canRetry(t)) return;
  t.status = "queued";
  t.error = undefined;
  t.attempt++;
  t.cancelRequested = false;
  t.phase = undefined;
  t.startedAt = Date.now();
  t.finishedAt = undefined;
  tryDispatch(id);
}

/** Cancel a transfer.
 *  - queued: not running, mark cancelled directly without any IPC.
 *  - running: raise the backend cancel flag; runTransfer's catch branch will
 *    flip the status when the streaming loop exits. */
export async function cancel(id: string): Promise<void> {
  const t = find(id);
  if (!t) return;
  if (t.status === "queued") {
    t.status = "cancelled";
    t.finishedAt = Date.now();
    return;
  }
  if (t.status !== "running" || t.cancelRequested || t.phase === "publishing") return;
  t.cancelRequested = true;
  await sendCancel(t);
}

async function sendCancel(t: Transfer): Promise<void> {
  const attempt = t.attempt;
  try {
    await invoke("sftp_cancel_transfer", { transferId: t.id });
  } catch (e) {
    if (t.status === "running" && t.attempt === attempt) {
      t.cancelRequested = false;
      toast.error(errMsg(e));
    }
  }
}

function isFinished(item: Transfer): boolean {
  return item.status !== "running" && item.status !== "queued";
}

function forget(item: Transfer): void {
  // Release the checkpoint and any folder grant without touching retained files.
  void invoke("sftp_forget_transfer", {
    transferId: item.id,
    ...(item.grantId === undefined ? {} : { grantId: item.grantId }),
  }).catch((error) => toast.error(errMsg(error)));
}

export function remove(id: string): void {
  const item = find(id);
  if (!item || !isFinished(item)) return;
  _list = _list.filter((t) => t.id !== id);
  forget(item);
}

export function clearFinished(): void {
  const finished = _list.filter(isFinished);
  _list = _list.filter((t) => !isFinished(t));
  for (const item of finished) forget(item);
}
