import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";

const mocks = vi.hoisted(() => ({ invoke: vi.fn(), listen: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("../stores/app.svelte.ts", () => ({
  connectedSessions: () => [{ sessionId: "ssh", tabId: "ssh-tab", type: "ssh" }],
  sessionIdForTab: () => "ssh", sidebarPosition: () => "left",
  closeDownloads: vi.fn(), downloadsActive: () => true,
}));
vi.mock("../stores/back.svelte.ts", () => ({ createScopedBackHandler: () => () => () => {} }));

import DownloadsScreen from "./DownloadsScreen.svelte";
import * as transfers from "../stores/transfers.svelte.ts";
import { setLocale, t } from "../i18n/index.svelte.ts";

function deferred<T = void>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}
const coded = (code: string, params: Record<string, string> = {}) => `__rssh_err__|${JSON.stringify({ code, params })}`;
let component: ReturnType<typeof mount> | undefined;
let progress: (event: { payload: Record<string, unknown> }) => void;
let releasePending: (() => void)[];

beforeEach(() => {
  setLocale("en");
  releasePending = [];
  mocks.invoke.mockReset().mockResolvedValue(undefined);
  mocks.listen.mockReset().mockImplementation(async (_channel: string, listener) => {
    progress = listener;
    return vi.fn();
  });
  component = mount(DownloadsScreen, { target: document.body });
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  mocks.invoke.mockResolvedValue(undefined);
  releasePending.forEach((release) => release());
  await vi.waitFor(() => expect(transfers.activeCount()).toBe(0));
  await transfers.clearFinished();
  document.body.innerHTML = "";
  setLocale("en");
});

const recoveries = [
  { code: "sftp_transfer_backup_preserved", params: { backup: "/local/owned.bak", part: "/local/owned.part" }, path: "/local/owned.bak", chinese: "旧内容保留在", endpointFailure: false },
  { code: "sftp_transfer_backup_location_unknown", params: { backup: "/local/uncertain.bak", part: "/local/owned.part", destination: "/local/file" }, path: "/local/uncertain.bak", chinese: "无法确认替换或清理结果", endpointFailure: false },
  { code: "sftp_transfer_backup_failed", params: { destination: "/local/original-retained", part: "/local/owned.part" }, path: "/local/original-retained", chinese: "已确认原文件仍在", endpointFailure: false },
  { code: "sftp_provider_recovery_preserved", params: { path: "content://provider/recovery-123" }, path: "content://provider/recovery-123", chinese: "文档提供器操作结果需要核对", endpointFailure: true },
];

describe("transfer recovery locations in the existing queue", () => {
  it.each(recoveries)("retains $code across early retry failure/cancellation, localizes it, and clears it only on success", async (recovery) => {
    const secondConnection = deferred<string>();
    const thirdConnection = deferred<string>();
    const completion = deferred();
    releasePending.push(() => secondConnection.resolve("sftp"), () => thirdConnection.resolve("sftp"), () => completion.resolve());
    let attempt = 0;
    mocks.invoke.mockImplementation(async (name: string) => {
      if (name === "sftp_connect_session") {
        attempt++;
        if (attempt === 2) return secondConnection.promise;
        if (attempt === 3) return thirdConnection.promise;
        return "sftp";
      }
      if (name === "sftp_download_to") {
        if (attempt === 1) throw coded(recovery.code, recovery.params);
        if (attempt === 2) throw coded("sftp_folder_required");
        return completion.promise;
      }
    });
    const id = await transfers.startDownload({ sessionId: "ssh", remotePath: "/remote/file", localPath: "/local/file" });
    await vi.waitFor(() => expect(transfers.list()[0].status).toBe("failed"));
    flushSync();
    expect(document.body.textContent).toContain(recovery.path);

    await transfers.retry(id);
    await vi.waitFor(() => expect(attempt).toBe(2));
    flushSync();
    expect(document.body.textContent).toContain(recovery.path);
    setLocale("zh");
    flushSync();
    expect(document.body.textContent).toContain(recovery.chinese);
    if (recovery.endpointFailure) secondConnection.resolve("sftp");
    else secondConnection.reject(coded("transfer_identity_changed"));
    await vi.waitFor(() => expect(transfers.list()[0].status).toBe("failed"));
    flushSync();
    expect(document.body.textContent).toContain(recovery.path);
    expect(document.body.textContent).toContain(t(recovery.endpointFailure ? "error.sftp_folder_required" : "error.transfer_identity_changed"));

    await transfers.retry(id);
    await vi.waitFor(() => expect(attempt).toBe(3));
    await transfers.cancel(id);
    thirdConnection.resolve("sftp");
    await vi.waitFor(() => expect(transfers.list()[0].status).toBe("cancelled"));
    flushSync();
    expect(document.body.textContent).toContain(recovery.path);
    expect(document.body.textContent).toContain(t("error.transfer_cancelled"));

    await transfers.retry(id);
    await vi.waitFor(() => expect(attempt).toBe(4));
    progress({ payload: { transferred: 100, total: 100, phase: "committing" } });
    flushSync();
    expect(document.body.textContent).toContain(recovery.path);
    completion.resolve();
    await vi.waitFor(() => expect(transfers.list()[0].status).toBe("done"));
    flushSync();
    expect(document.body.textContent).not.toContain(recovery.path);
    expect(document.body.textContent).not.toContain("__rssh_err__");
  });
});
