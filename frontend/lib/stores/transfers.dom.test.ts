import { beforeEach, describe, expect, it, vi } from "vitest";

const { invoke, listen } = vi.hoisted(() => ({
  invoke: vi.fn(), listen: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen }));
const paths = { sessionId: "ssh-1", localPath: "/local/file", remotePath: "/remote/file" };

beforeEach(() => {
  vi.resetAllMocks();
  vi.resetModules();
  listen.mockResolvedValue(() => {});
});

describe("SFTP transfer retries", () => {
  it.each(["download", "upload"] as const)("keeps the %s checkpoint and uses a reconnected SSH session", async (direction) => {
    const queue = await import("./transfers.svelte.ts");
    const command = direction === "download" ? "sftp_download_to" : "sftp_upload_from";
    let finish!: () => void;
    let attempt = 0;
    let connection = 0;
    invoke.mockImplementation(async (name) => {
      if (name === "sftp_connect_session") return "sftp-" + ++connection;
      if (name !== command) return;
      if (attempt++ === 0) {
        listen.mock.lastCall![1]({ payload: { transferred: 48, total: 100 } });
        throw new Error("disconnected");
      }
      await new Promise<void>((resolve) => { finish = resolve; });
    });

    const id = await (direction === "download" ? queue.startDownload(paths) : queue.startUpload(paths));
    await vi.waitFor(() => expect(queue.list()[0].status).toBe("failed"));
    expect(queue.list()[0].transferred).toBe(48);
    expect(invoke).toHaveBeenCalledWith("sftp_connect_session", { sessionId: "ssh-1", transferId: id });
    await queue.retry(id);
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith(command, {
      originalSessionId: "ssh-1", sftpId: "sftp-2", transferId: id,
      localPath: paths.localPath, remotePath: paths.remotePath,
    }));
    expect(queue.list()[0]).toMatchObject({ status: "running", transferred: 48, checking: true });
    listen.mock.lastCall![1]({ payload: { transferred: 52, total: 100 } });
    expect(queue.list()[0]).toMatchObject({ transferred: 52, checking: false });
    listen.mock.lastCall![1]({ payload: { transferred: 100, total: 100 } });
    finish();
    await vi.waitFor(() => expect(queue.list()[0].status).toBe("done"));
    await queue.clearFinished();
    expect(queue.list()).toHaveLength(0);
    expect(invoke).not.toHaveBeenCalledWith("sftp_forget_transfer", expect.anything());
  });

  it("keeps a failed entry off unrelated hosts and releases its checkpoint on removal", async () => {
    const queue = await import("./transfers.svelte.ts");
    let connections = 0;
    invoke.mockImplementation(async (name) => {
      if (name === "sftp_connect_session") {
        if (++connections > 1) throw new Error("Reconnect the original host");
        return "sftp-ssh-1";
      }
      if (name === "sftp_upload_from") throw new Error("disconnected");
      if (name === "sftp_forget_transfer") return "/remote/retained.part";
    });
    const id = await queue.startUpload(paths);
    await vi.waitFor(() => expect(queue.list()[0].status).toBe("failed"));
    await queue.retry(id);
    await vi.waitFor(() => expect(queue.list()[0].status).toBe("failed"));
    expect(invoke.mock.calls.filter(([name]) => name === "sftp_upload_from")).toHaveLength(1);
    await queue.remove(id);
    expect(invoke).toHaveBeenCalledWith("sftp_forget_transfer", { transferId: id });
    const { toasts } = await import("./toast.svelte.ts");
    expect(toasts().some((item) => item.message.includes("/remote/retained.part"))).toBe(true);
    expect(queue.list()).toHaveLength(0);
  });

  it.each([
    ["sftp_replace_backup", "failed"],
    ["sftp_replace_uncertain", "failed"],
    ["sftp_staging_retained", "failed"],
    ["sftp_backup_cleanup", "done"],
  ])("shows %s with its path and prevents a second publish", async (code, status) => {
    const queue = await import("./transfers.svelte.ts");
    invoke.mockImplementation(async (name) => {
      if (name === "sftp_connect_session") return "sftp-1";
      if (name === "sftp_upload_from") {
        throw "__rssh_err__|" + JSON.stringify({ code, params: { path: "/remote/file.bak", err: "denied" } });
      }
    });
    const id = await queue.startUpload(paths);
    await vi.waitFor(() => expect(queue.list()[0].status).toBe(status));
    expect(queue.list()[0].error).toContain("/remote/file.bak");
    await queue.retry(id);
    expect(invoke.mock.calls.filter(([name]) => name === "sftp_upload_from")).toHaveLength(1);
    await queue.remove(id);
    expect(invoke).toHaveBeenCalledWith("sftp_forget_transfer", { transferId: id });
  });

  it("cancels a transfer while its SFTP subsystem is still opening", async () => {
    const queue = await import("./transfers.svelte.ts");
    let connected!: (id: string) => void;
    invoke.mockImplementation(async (name) => {
      if (name === "sftp_connect_session") return new Promise((resolve) => { connected = resolve; });
    });
    const id = await queue.startUpload(paths);
    await vi.waitFor(() => expect(connected).toBeTypeOf("function"));
    await queue.cancel(id);
    connected("sftp-late");
    await vi.waitFor(() => expect(queue.list()[0].status).toBe("cancelled"));
    expect(invoke.mock.calls.some(([name]) => name === "sftp_upload_from")).toBe(false);
    expect(invoke).toHaveBeenCalledWith("sftp_close", { sftpId: "sftp-late" });
  });
});
