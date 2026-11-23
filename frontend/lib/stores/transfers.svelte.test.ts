import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(), listen: vi.fn(), connectedSessions: vi.fn(), sessionIdForTab: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: mocks.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: mocks.listen }));
vi.mock("./app.svelte.ts", () => ({ connectedSessions: mocks.connectedSessions, sessionIdForTab: mocks.sessionIdForTab }));

function deferred<T = void>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((done, fail) => { resolve = done; reject = fail; });
  return { promise, resolve, reject };
}

const coded = (code: string, params = {}) => `__rssh_err__|${JSON.stringify({ code, params })}`;
const args = { sessionId: "ssh-original", remotePath: "/remote/file", localPath: "/local/file" };
let progress: Map<string, (event: { payload: Record<string, unknown> }) => void>;
let stopped: Map<string, ReturnType<typeof vi.fn>>;

beforeEach(() => {
  vi.resetModules();
  progress = new Map();
  stopped = new Map();
  mocks.invoke.mockReset().mockImplementation(async (command: string) => command === "sftp_connect_session" ? "sftp-new" : undefined);
  mocks.listen.mockReset().mockImplementation(async (channel: string, listener) => {
    progress.set(channel, listener);
    const stop = vi.fn();
    stopped.set(channel, stop);
    return stop;
  });
  mocks.connectedSessions.mockReset().mockReturnValue([{ sessionId: args.sessionId, tabId: "ssh-tab", type: "ssh", label: "Host" }]);
  mocks.sessionIdForTab.mockReset().mockReturnValue(args.sessionId);
});

async function load() { return import("./transfers.svelte.ts"); }
function emit(id: string, payload: Record<string, unknown>) { progress.get(`sftp:progress:${id}`)!({ payload }); }

for (const kind of ["download", "upload"] as const) {
  describe(`${kind} resume in the existing queue`, () => {
    const command = kind === "download" ? "sftp_download_to" : "sftp_upload_from";
    it("keeps its ID and bytes through reconnect, then displays verified progress until commit succeeds", async () => {
      const attempts = [deferred(), deferred()];
      let attempt = 0;
      mocks.invoke.mockImplementation(async (name: string) => {
        if (name === "sftp_connect_session") return `sftp-${attempt}`;
        if (name === command) return attempts[attempt++].promise;
      });
      const store = await load();
      const id = await (kind === "download" ? store.startDownload(args) : store.startUpload(args));
      await vi.waitFor(() => expect(attempt).toBe(1));
      emit(id, { transferred: 400, total: 1000, phase: "transferring" });
      attempts[0].reject(coded("sftp_io_failed", { op: "write", err: "disconnected" }));
      await vi.waitFor(() => expect(store.list()[0].status).toBe("failed"));
      expect(store.list()[0].transferred).toBe(400);
      expect(stopped.get(`sftp:progress:${id}`)).toHaveBeenCalledOnce();

      mocks.sessionIdForTab.mockReturnValue("ssh-reconnected");
      await store.retry(id);
      expect(store.list()[0]).toMatchObject({ id, transferred: 400, phase: "connecting" });
      await vi.waitFor(() => expect(attempt).toBe(2));
      expect(mocks.invoke).toHaveBeenCalledWith("sftp_connect_session", { sessionId: "ssh-reconnected", transferId: id });
      expect(mocks.invoke.mock.calls.filter(([name]) => name === command).map(([, params]) => params.transferId)).toEqual([id, id]);

      emit(id, { transferred: 320, total: 1000, phase: "verifying", verified: 160, verificationTotal: 320 });
      expect(store.list()[0]).toMatchObject({ transferred: 320, phase: "verifying", verified: 160, verificationTotal: 320 });
      emit(id, { transferred: 1000, total: 1000, phase: "committing" });
      expect(store.list()[0]).toMatchObject({ status: "running", phase: "committing", transferred: 1000 });
      attempts[1].resolve();
      await vi.waitFor(() => expect(store.list()[0].status).toBe("done"));
      emit(id, { transferred: 0, total: 1000 });
      expect(store.list()[0].transferred).toBe(1000);
      await store.retry(id);
      expect(attempt).toBe(2);
      expect(mocks.invoke.mock.calls.filter(([name]) => name === "sftp_register_transfer")).toEqual([["sftp_register_transfer", {
        sessionId: args.sessionId, transferId: id,
      }]]);
    });

    it("waits for original-session registration before publishing or dispatching the row", async () => {
      const registration = deferred();
      const transfer = deferred();
      mocks.invoke.mockImplementation(async (name: string) => {
        if (name === "sftp_register_transfer") return registration.promise;
        if (name === "sftp_connect_session") return "sftp";
        if (name === command) return transfer.promise;
      });
      const store = await load();
      const starting = kind === "download" ? store.startDownload(args) : store.startUpload(args);
      expect(mocks.invoke).toHaveBeenCalledExactlyOnceWith("sftp_register_transfer", { sessionId: args.sessionId, transferId: expect.any(String) });
      expect(store.list()).toHaveLength(0);
      expect(mocks.listen).not.toHaveBeenCalled();
      // The originating tab is captured before the registration await.
      mocks.connectedSessions.mockReturnValue([]);
      registration.resolve();
      const id = await starting;
      expect(store.list()[0]).toMatchObject({ id, tabId: "ssh-tab" });
      await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith(command, expect.objectContaining({ transferId: id })));
      transfer.resolve();
      await vi.waitFor(() => expect(store.list()[0].status).toBe("done"));
    });

    it("rejects failed registration without publishing a row or opening a channel", async () => {
      const error = coded("transfer_identity_changed");
      mocks.invoke.mockRejectedValue(error);
      const store = await load();
      await expect(kind === "download" ? store.startDownload(args) : store.startUpload(args)).rejects.toBe(error);
      expect(store.list()).toHaveLength(0);
      expect(mocks.listen).not.toHaveBeenCalled();
      expect(mocks.invoke).toHaveBeenCalledExactlyOnceWith("sftp_register_transfer", { sessionId: args.sessionId, transferId: expect.any(String) });
    });

    it("retains its original registration when the tab changes host while waiting for a queue slot", async () => {
      const running = deferred();
      const registered = new Map<string, string>();
      mocks.invoke.mockImplementation(async (name: string, params: { sessionId?: string; transferId: string }) => {
        if (name === "sftp_register_transfer") registered.set(params.transferId, params.sessionId!);
        if (name === "sftp_connect_session") {
          if (registered.get(params.transferId) !== params.sessionId) throw coded("transfer_identity_changed");
          return "sftp";
        }
        if (name === command) return running.promise;
      });
      const store = await load();
      await store.setMaxConcurrent(1);
      const first = await (kind === "download" ? store.startDownload(args) : store.startUpload(args));
      await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith(command, expect.objectContaining({ transferId: first })));
      const queued = await (kind === "download" ? store.startDownload(args) : store.startUpload(args));
      expect(store.list().find((item) => item.id === queued)?.status).toBe("queued");
      expect(registered.get(queued)).toBe(args.sessionId);
      mocks.sessionIdForTab.mockReturnValue("ssh-other-host");
      running.resolve();
      await vi.waitFor(() => expect(store.list().find((item) => item.id === queued)?.status).toBe("failed"));
      expect(store.list().find((item) => item.id === queued)?.error).toContain("original verified host");
      expect(mocks.invoke.mock.calls.filter(([name]) => name === command)).toHaveLength(1);
      expect(mocks.invoke.mock.calls.filter(([name, params]) => name === "sftp_register_transfer" && params.transferId === queued)).toEqual([["sftp_register_transfer", {
        sessionId: args.sessionId, transferId: queued,
      }]]);
    });

    it("does not rebind to another host after an early channel failure before the first stream", async () => {
      const registered = new Map<string, string>();
      mocks.invoke.mockImplementation(async (name: string, params: { sessionId: string; transferId: string }) => {
        if (name === "sftp_register_transfer") registered.set(params.transferId, params.sessionId);
        if (name === "sftp_connect_session") {
          if (registered.get(params.transferId) !== params.sessionId) throw coded("transfer_identity_changed");
          throw coded("sftp_io_failed", { op: "connect", err: "disconnected" });
        }
      });
      const store = await load();
      const id = await (kind === "download" ? store.startDownload(args) : store.startUpload(args));
      await vi.waitFor(() => expect(store.list()[0].status).toBe("failed"));
      mocks.sessionIdForTab.mockReturnValue("ssh-other-host");
      await store.retry(id);
      await vi.waitFor(() => expect(store.list()[0].status).toBe("failed"));
      expect(store.list()[0].error).toContain("original verified host");
      expect(mocks.invoke.mock.calls.filter(([name]) => name === command)).toHaveLength(0);
      expect(mocks.invoke.mock.calls.filter(([name]) => name === "sftp_register_transfer")).toEqual([["sftp_register_transfer", {
        sessionId: args.sessionId, transferId: id,
      }]]);
    });
  });
}

describe("queue lifetime and recoverable errors", () => {
  it("cancels connecting locally, closes the late subsystem, and starts exactly one retry", async () => {
    const connecting = deferred<string>();
    mocks.invoke.mockImplementation((name: string) => name === "sftp_connect_session" ? connecting.promise : Promise.resolve());
    const store = await load();
    const id = await store.startDownload(args);
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("sftp_connect_session", expect.any(Object)));
    await store.cancel(id);
    connecting.resolve("sftp-late");
    await vi.waitFor(() => expect(store.list()[0].status).toBe("cancelled"));
    expect(store.list()[0].error).toBe("Transfer cancelled");
    expect(mocks.invoke).toHaveBeenCalledWith("sftp_close", { sftpId: "sftp-late" });
    expect(mocks.invoke.mock.calls.some(([name]) => name === "sftp_download_to")).toBe(false);
    expect(mocks.invoke.mock.calls.some(([name]) => name === "sftp_cancel_transfer")).toBe(false);

    await store.retry(id);
    await vi.waitFor(() => expect(store.list()[0].status).toBe("done"));
    expect(mocks.invoke.mock.calls.filter(([name]) => name === "sftp_download_to")).toEqual([["sftp_download_to", {
      sftpId: "sftp-late", remotePath: args.remotePath, localPath: args.localPath, transferId: id,
    }]]);
  });

  it("does not connect after cancellation while the listener is attaching", async () => {
    const listening = deferred<() => void>();
    mocks.listen.mockReturnValue(listening.promise);
    const store = await load();
    const id = await store.startUpload(args);
    await store.cancel(id);
    const stop = vi.fn();
    listening.resolve(stop);
    await vi.waitFor(() => expect(store.list()[0].status).toBe("cancelled"));
    expect(stop).toHaveBeenCalledOnce();
    expect(mocks.invoke.mock.calls.some(([name]) => name === "sftp_connect_session")).toBe(false);
    expect(mocks.invoke.mock.calls.some(([name]) => name === "sftp_cancel_transfer")).toBe(false);
  });

  it("sends cancellation after invoking a transfer even before its first progress event", async () => {
    const transfer = deferred();
    mocks.invoke.mockImplementation(async (name: string) => {
      if (name === "sftp_connect_session") return "sftp";
      if (name === "sftp_upload_from") return transfer.promise;
      if (name === "sftp_cancel_transfer") transfer.reject(coded("transfer_cancelled"));
    });
    const store = await load();
    const id = await store.startUpload(args);
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("sftp_upload_from", expect.any(Object)));
    expect(store.list()[0].phase).toBe("connecting");
    await store.cancel(id);
    await vi.waitFor(() => expect(store.list()[0].status).toBe("cancelled"));
    expect(mocks.invoke).toHaveBeenCalledWith("sftp_cancel_transfer", { transferId: id });
  });

  for (const rejected of [false, true]) {
    it(`waits for a late ${rejected ? "failed" : "successful"} cancellation before opening the retry channel`, async () => {
      const firstTransfer = deferred();
      const cancellation = deferred();
      const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
      let streams = 0;
      const order: string[] = [];
      mocks.invoke.mockImplementation(async (name: string) => {
        if (name === "sftp_connect_session") {
          order.push("connect");
          return "sftp";
        }
        if (name === "sftp_download_to" && ++streams === 1) return firstTransfer.promise;
        if (name === "sftp_cancel_transfer") {
          try { await cancellation.promise; } finally { order.push("cancel settled"); }
        }
      });
      const store = await load();
      const id = await store.startDownload(args);
      await vi.waitFor(() => expect(streams).toBe(1));
      const cancel = store.cancel(id);
      const duplicateCancel = store.cancel(id);
      firstTransfer.reject(coded("sftp_transfer_backup_preserved", { backup: "/local/retained.bak", part: "/local/retained.part" }));
      await vi.waitFor(() => expect(store.list()[0].status).toBe("failed"));
      const retry = store.retry(id);
      const duplicateRetry = store.retry(id);
      await new Promise((resolve) => setTimeout(resolve, 0));
      expect(order).toEqual(["connect"]);
      expect(store.list()[0].recovery?.params?.backup).toBe("/local/retained.bak");

      if (rejected) cancellation.reject(new Error("late cancel failure"));
      else cancellation.resolve();
      await Promise.all([cancel, duplicateCancel, retry, duplicateRetry]);
      await vi.waitFor(() => expect(store.list()[0].status).toBe("done"));
      expect(order).toEqual(["connect", "cancel settled", "connect"]);
      expect(streams).toBe(2);
      expect(store.list()[0].error).toBeUndefined();
      expect(mocks.invoke.mock.calls.filter(([name]) => name === "sftp_cancel_transfer")).toHaveLength(1);
      consoleError.mockRestore();
    });
  }

  it("settles a late cancellation before forgetting an inactive record", async () => {
    const transfer = deferred();
    const cancellation = deferred();
    mocks.invoke.mockImplementation(async (name: string) => {
      if (name === "sftp_connect_session") return "sftp";
      if (name === "sftp_upload_from") return transfer.promise;
      if (name === "sftp_cancel_transfer") return cancellation.promise;
    });
    const store = await load();
    const id = await store.startUpload(args);
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("sftp_upload_from", expect.any(Object)));
    const cancel = store.cancel(id);
    transfer.reject(new Error("disconnected"));
    await vi.waitFor(() => expect(store.list()[0].status).toBe("failed"));
    const clear = store.clearFinished();
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(mocks.invoke.mock.calls.some(([name]) => name === "sftp_forget_transfer")).toBe(false);
    cancellation.resolve();
    await Promise.all([cancel, clear]);
    expect(mocks.invoke).toHaveBeenCalledWith("sftp_forget_transfer", { transferId: id });
    expect(store.list()).toHaveLength(0);
  });

  it("keeps the recovery path visible when replacement fails after all bytes arrive", async () => {
    const transfer = deferred();
    mocks.invoke.mockImplementation(async (name: string) => name === "sftp_download_to" ? transfer.promise : "sftp");
    const store = await load();
    const id = await store.startDownload(args);
    await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith("sftp_download_to", expect.any(Object)));
    emit(id, { transferred: 100, total: 100, phase: "committing" });
    transfer.reject(coded("sftp_transfer_backup_preserved", { backup: "/local/file.own.bak", part: "/local/file.own.part", err: "transfer_cancelled" }));
    await vi.waitFor(() => expect(store.list()[0].status).toBe("failed"));
    expect(store.list()[0]).toMatchObject({ transferred: 100, total: 100 });
    expect(store.list()[0].recovery).toEqual({ code: "sftp_transfer_backup_preserved", params: {
      backup: "/local/file.own.bak", part: "/local/file.own.part", err: "transfer_cancelled",
    } });
    expect(store.list()[0].error).toBeUndefined();
  });

  it("promotes queued work when attaching a progress listener fails", async () => {
    const firstListener = deferred<() => void>();
    mocks.listen.mockReturnValueOnce(firstListener.promise);
    const store = await load();
    await store.setMaxConcurrent(1);
    const first = await store.startDownload(args);
    const second = await store.startUpload({ ...args, remotePath: "/other" });
    expect(store.list().find((item) => item.id === second)?.status).toBe("queued");
    firstListener.reject(new Error("events unavailable"));
    await vi.waitFor(() => expect(store.list().find((item) => item.id === second)?.status).toBe("done"));
    expect(store.list().find((item) => item.id === first)).toMatchObject({ status: "failed", error: "events unavailable" });
  });

  it("forgets only inactive records and keeps failed cleanup visible", async () => {
    const running = deferred();
    let streams = 0;
    mocks.invoke.mockImplementation(async (name: string) => {
      if (name === "sftp_connect_session") return "sftp";
      if (name === "sftp_download_to" && ++streams === 2) return running.promise;
      if (name === "sftp_forget_transfer") throw new Error("metadata unavailable");
    });
    const store = await load();
    await store.setMaxConcurrent(1);
    const done = await store.startDownload(args);
    await vi.waitFor(() => expect(store.list()[0].status).toBe("done"));
    const active = await store.startDownload(args);
    const queued = await store.startDownload(args);
    await store.remove(active);
    await store.remove(queued);
    await store.clearFinished();
    expect(mocks.invoke.mock.calls.filter(([name]) => name === "sftp_forget_transfer")).toEqual([["sftp_forget_transfer", { transferId: done }]]);
    expect(store.list().find((item) => item.id === done)?.error).toBe("metadata unavailable");
    mocks.invoke.mockImplementation(async (name: string) => name === "sftp_connect_session" ? "sftp" : undefined);
    await store.remove(done);
    expect(store.list().some((item) => item.id === done)).toBe(false);
    await store.cancel(queued);
    running.resolve();
    await vi.waitFor(() => expect(store.activeCount()).toBe(0));
  });
});
