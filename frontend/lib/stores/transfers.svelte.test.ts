import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
const listen = vi.hoisted(() => vi.fn());
const sessionIdForTab = vi.hoisted(() => vi.fn());
const tabs = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));
vi.mock("./app.svelte.ts", () => ({ sessionIdForTab, tabs }));

const origin = { tabId: "tab", profileId: "profile", sessionId: "original" };
const paths = { remotePath: "/remote/data", localPath: "/local/data" };

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

beforeEach(() => {
  vi.resetModules();
  invoke.mockReset();
  listen.mockReset().mockResolvedValue(vi.fn());
  tabs.mockReset().mockReturnValue([]);
  sessionIdForTab.mockReset();
});

describe("SFTP transfer queue", () => {
  it.each(["download", "upload"] as const)("resumes a %s on a verified replacement session with the original transfer ID", async kind => {
    const queue = await import("./transfers.svelte.ts");
    const first = deferred<number>();
    const second = deferred<number>();
    const command = kind === "download" ? "sftp_download_to" : "sftp_upload_from";
    const attempts = [first, second];
    invoke.mockImplementation(async (name, args) => {
      if (name === "sftp_connect_session") return "child-" + args.sessionId;
      if (name === command) return attempts.shift()!.promise;
    });

    const start = kind === "download" ? queue.startDownload : queue.startUpload;
    const id = await start({ ...origin, ...paths });
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith(command, {
      sftpId: "child-original", transferId: id, originalSessionId: "original", ...paths,
    }));
    const firstProgress = listen.mock.calls[0][1];
    firstProgress({ payload: { transferred: 12, total: 30 } });
    first.reject("connection closed");
    await vi.waitFor(() => expect(queue.list()[0].status).toBe("failed"));

    // A reconnect can occur on the original tab, or via a new tab for the
    // same profile. The backend still receives the original session identity.
    if (kind === "upload") {
      tabs.mockReturnValue([{ type: "ssh", id: "reopened", meta: { profileId: origin.profileId } }]);
      sessionIdForTab.mockImplementation(id => id === "reopened" ? "reconnected" : undefined);
    } else {
      sessionIdForTab.mockReturnValue("reconnected");
    }
    await queue.retry(id);
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith(command, {
      sftpId: "child-reconnected", transferId: id, originalSessionId: "original", ...paths,
    }));
    firstProgress({ payload: { transferred: 0, total: 30 } });
    expect(queue.list()[0].transferred).toBe(12);
    const resumedProgress = listen.mock.calls[1][1];
    resumedProgress({ payload: { transferred: 12, total: 30, phase: "verifying" } });
    expect(queue.list()[0]).toMatchObject({ transferred: 12, phase: "verifying" });
    resumedProgress({ payload: { transferred: 11, total: 30 } });
    expect(queue.list()[0]).toMatchObject({ transferred: 11, phase: "transferring" });
    second.resolve(32);
    await vi.waitFor(() => expect(queue.list()[0]).toMatchObject({ status: "done", transferred: 32, total: 32 }));
    queue.remove(id);
    expect(invoke).toHaveBeenCalledWith("sftp_forget_transfer", { transferId: id });
  });

  it.each([
    ["sftp_replace_failed", "failed"],
    ["sftp_backup_cleanup_failed", "done"],
  ])("retains the recovery path when publication reports %s", async (code, status) => {
    const queue = await import("./transfers.svelte.ts");
    const message = "__rssh_err__|" + JSON.stringify({
      code, params: { target: "/target", backup: "/original.bak", err: "permission denied" },
    });
    invoke.mockImplementation(async name => {
      if (name === "sftp_connect_session") return "sftp";
      if (name === "sftp_download_to") throw message;
    });
    const id = await queue.startDownload({ ...origin, ...paths });
    await vi.waitFor(() => expect(queue.list()[0]).toMatchObject({ status, error: message }));
    expect(queue.canRetry(queue.list()[0])).toBe(false);
    await queue.retry(id);
    expect(invoke.mock.calls.filter(([name]) => name === "sftp_download_to")).toHaveLength(1);
  });

  it("retries the chosen local path after its directory permissions change", async () => {
    const queue = await import("./transfers.svelte.ts");
    let granted = false;
    invoke.mockImplementation(async name => {
      if (name === "sftp_connect_session") return "sftp";
      if (name === "sftp_download_to") {
        if (!granted) throw '__rssh_err__|{"code":"sftp_provider_siblings_unavailable","params":{}}';
        return 42;
      }
    });
    const id = await queue.startDownload({ sessionId: "original", ...paths });
    await vi.waitFor(() => expect(queue.list()[0].status).toBe("failed"));
    expect(queue.canRetry(queue.list()[0])).toBe(true);
    granted = true;
    await queue.retry(id);
    await vi.waitFor(() => expect(queue.list()[0]).toMatchObject({ status: "done", transferred: 42 }));
  });

  it("reuses a folder grant across download attempts and releases it with the finished task", async () => {
    const queue = await import("./transfers.svelte.ts");
    const download = vi.fn().mockRejectedValueOnce("connection closed").mockResolvedValueOnce(32);
    invoke.mockImplementation(async (name, args) => {
      if (name === "sftp_connect_session") return "sftp";
      if (name === "sftp_download_to") return download(args);
    });
    const id = await queue.startDownload({ sessionId: "original", ...paths, grantId: 0 });
    await vi.waitFor(() => expect(queue.list()[0].status).toBe("failed"));
    expect(invoke.mock.calls.some(([name]) => name === "sftp_forget_transfer")).toBe(false);
    await queue.retry(id);
    await vi.waitFor(() => expect(queue.list()[0].status).toBe("done"));
    expect(download).toHaveBeenCalledTimes(2);
    for (const [args] of download.mock.calls) {
      expect(args).toMatchObject({ transferId: id, localPath: paths.localPath, grantId: 0 });
    }
    queue.clearFinished();
    expect(invoke).toHaveBeenCalledWith("sftp_forget_transfer", { transferId: id, grantId: 0 });
  });

  it.each(["download", "upload"] as const)("keeps a %s backup candidate if IPC closes during the final rename", async kind => {
    const queue = await import("./transfers.svelte.ts");
    const { errCoded } = await import("../i18n/index.svelte.ts");
    const publishing = deferred<number>();
    const command = kind === "download" ? "sftp_download_to" : "sftp_upload_from";
    invoke.mockImplementation(async name => name === command ? publishing.promise : "sftp");
    const start = kind === "download" ? queue.startDownload : queue.startUpload;
    const id = await start({ ...origin, ...paths });
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith(command, expect.any(Object)));
    const target = kind === "download" ? paths.localPath : paths.remotePath;
    const backup = target + ".backup.bak";
    listen.mock.calls[0][1]({ payload: { phase: "publishing", transferred: 10, total: 10, backupPath: backup } });
    expect(queue.list()[0].phase).toBe("publishing");
    await queue.cancel(id);
    expect(invoke.mock.calls.some(([name]) => name === "sftp_cancel_transfer")).toBe(false);
    publishing.reject(new Error("rssh server connection closed"));
    await vi.waitFor(() => expect(queue.list()[0].status).toBe("failed"));
    expect(errCoded(queue.list()[0].error)).toEqual({ code: "sftp_commit_uncertain", params: { path: target, backup } });
    expect(queue.canRetry(queue.list()[0])).toBe(false);
  });

  it("repeats an early cancel when the backend acknowledges the attempt", async () => {
    const queue = await import("./transfers.svelte.ts");
    const transfer = deferred<number>();
    invoke.mockImplementation(async name => name === "sftp_upload_from" ? transfer.promise : "sftp");
    const id = await queue.startUpload({ ...origin, ...paths });
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("sftp_upload_from", expect.any(Object)));
    await queue.cancel(id);
    const emit = listen.mock.calls[0][1];
    emit({ payload: { transferred: 0, total: 11, partialPath: "/remote/test.part" } });
    emit({ payload: { transferred: 1, total: 11 } });
    expect(invoke.mock.calls.filter(([name]) => name === "sftp_cancel_transfer")).toHaveLength(2);
    transfer.reject('__rssh_err__|{"code":"transfer_cancelled"}');
    await vi.waitFor(() => expect(queue.list()[0]).toMatchObject({ status: "cancelled", partialPath: "/remote/test.part" }));
  });

  it("cancels a pending connection before it can start writing", async () => {
    const queue = await import("./transfers.svelte.ts");
    const opening = deferred<string>();
    invoke.mockImplementation(async name => name === "sftp_connect_session" ? opening.promise : undefined);
    const id = await queue.startUpload({ ...origin, ...paths });
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("sftp_connect_session", { sessionId: "original" }));
    await queue.cancel(id);
    opening.resolve("sftp");
    await vi.waitFor(() => expect(queue.list()[0].status).toBe("cancelled"));
    expect(invoke).toHaveBeenCalledWith("sftp_close", { sftpId: "sftp" });
    expect(invoke.mock.calls.some(([name]) => name === "sftp_upload_from")).toBe(false);
  });
});
