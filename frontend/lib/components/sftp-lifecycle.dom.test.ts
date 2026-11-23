import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, tick, unmount } from "svelte";
import SftpBrowser from "./SftpBrowser.svelte";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

let component: ReturnType<typeof mount> | undefined;
let target: HTMLDivElement;

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

beforeEach(() => {
  invokeMock.mockReset().mockImplementation(async (command: string) => {
    if (command === "sftp_connect_session") return "sftp:ready";
    if (command === "sftp_home") return "/home/test";
    return [];
  });
  target = document.createElement("div");
  document.body.append(target);
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.innerHTML = "";
});

describe("SFTP browser connection lifetime", () => {
  it.each([
    ["sftp_connect_session", { sessionId: "ssh:test" }],
    ["sftp_connect", { host: "localhost", port: "22", username: "test", authType: "agent" }],
  ] as const)("closes a late %s result after the panel unmounts", async (command, meta) => {
    const connect = deferred<string>();
    invokeMock.mockImplementation((name: string) => name === command ? connect.promise : Promise.resolve([]));
    component = mount(SftpBrowser, { target, props: { meta } });
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith(command, expect.any(Object)));

    await unmount(component);
    component = undefined;
    connect.resolve("sftp:late");

    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith("sftp_close", { sftpId: "sftp:late" }));
    expect(invokeMock.mock.calls.filter(([name]) => name === "sftp_close")).toHaveLength(1);
    expect(invokeMock.mock.calls.some(([name]) => name === "sftp_home" || name === "sftp_list")).toBe(false);
  });

  it("does not list a closed subsystem after a pending home lookup completes", async () => {
    const home = deferred<string>();
    invokeMock.mockImplementation((command: string) => {
      if (command === "sftp_connect_session") return Promise.resolve("sftp:ready");
      if (command === "sftp_home") return home.promise;
      return Promise.resolve([]);
    });
    component = mount(SftpBrowser, { target, props: { meta: { sessionId: "ssh:test" } } });
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith("sftp_home", { sftpId: "sftp:ready" }));

    await unmount(component);
    component = undefined;
    home.resolve("/home/test");
    await home.promise;
    await tick();

    expect(invokeMock.mock.calls.filter(([name]) => name === "sftp_close")).toEqual([
      ["sftp_close", { sftpId: "sftp:ready" }],
    ]);
    expect(invokeMock.mock.calls.some(([name]) => name === "sftp_list")).toBe(false);
  });
});

describe("SFTP folder grant", () => {
  it("selects a directory for a single mobile download and passes its resolved location to the queue", async () => {
    const app = await import("../stores/app.svelte.ts");
    const transfers = await import("../stores/transfers.svelte.ts");
    const capabilities = vi.spyOn(app, "capabilities").mockReturnValue({
      ...app.capabilities(), downloadDirectoryRequired: true, directoryTransfer: false,
    });
    const start = vi.spyOn(transfers, "startDownload").mockResolvedValue("transfer-1");
    try {
      invokeMock.mockImplementation(async (command: string) => {
        if (command === "sftp_connect_session") return "sftp-1";
        if (command === "sftp_home") return "/home/test";
        if (command === "sftp_list") return [{ name: "report.txt", is_dir: false, size: 13, mtime: 0 }];
        if (command === "sftp_pick_folder") return { name: "Documents", location: "tree-token" };
        if (command === "resolve_local_paths") return [
          { status: "ready", relative_path: "report.txt", location: "resolved-token" },
        ];
      });
      component = mount(SftpBrowser, { target, props: { meta: { sessionId: "ssh-1" } } });
      await vi.waitFor(() => expect(target.querySelector(".file-name")).not.toBeNull());
      const fileList = target.querySelector(".file-list")!;
      expect(fileList.classList.contains("no-selection")).toBe(false);
      const download = target.querySelector<HTMLButtonElement>(".header > button:last-child")!;
      expect(download.disabled).toBe(true);
      fileList.querySelector<HTMLInputElement>('.file-row:not(.file-header) input[type="checkbox"]')!.click();
      await tick();
      expect(download.disabled).toBe(false);
      target.querySelector<HTMLButtonElement>(".file-name")!.click();
      await vi.waitFor(() => expect(start).toHaveBeenCalledExactlyOnceWith({
        sessionId: "ssh-1", remotePath: "/home/test/report.txt",
        localPath: "resolved-token", localLabel: "Documents/report.txt", sizeHint: 13,
      }));
      expect(invokeMock).toHaveBeenCalledWith("sftp_pick_folder", { write: true });
      expect(invokeMock).toHaveBeenCalledWith("resolve_local_paths", {
        localRoot: "tree-token", relativePaths: ["report.txt"], write: true,
      });
      expect(invokeMock.mock.calls.some(([name]) => name === "sftp_pick_save_path")).toBe(false);
    } finally {
      capabilities.mockRestore();
      start.mockRestore();
    }
  });
});
