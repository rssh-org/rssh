import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, tick, unmount } from "svelte";
import DownloadsScreen from "./DownloadsScreen.svelte";
import * as app from "../stores/app.svelte.ts";
import * as transfers from "../stores/transfers.svelte.ts";
import { locale, setLocale, t } from "../i18n/index.svelte.ts";
import SftpBrowser from "./SftpBrowser.svelte";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));

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
  transfers.clearFinished();
  vi.restoreAllMocks();
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

describe("single-file download folder access", () => {
  const originalLocale = locale();
  const permissionError = "__rssh_err__|" + JSON.stringify({
    code: "sftp_provider_siblings_unavailable", params: { action: "reselect" },
  });

  afterEach(() => setLocale(originalLocale));

  async function clickRemoteFile(pickSavePath: () => string) {
    vi.spyOn(app, "capabilities").mockReturnValue({ ...app.capabilities(), directoryTransfer: false });
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "sftp_connect_session") return "sftp";
      if (command === "sftp_home") return "/";
      if (command === "sftp_list") return [{ name: "report.txt", size: 12, is_dir: false }];
      if (command === "sftp_pick_save_path") return pickSavePath();
      if (command === "sftp_download_to") throw permissionError;
      return null;
    });
    component = mount(SftpBrowser, { target, props: { meta: { tabId: "tab", sessionId: "ssh" } } });
    await vi.waitFor(() => expect(target.querySelector(".file-name")).not.toBeNull());
    target.querySelector<HTMLButtonElement>(".file-name")!.click();
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith("sftp_pick_save_path", { defaultName: "report.txt" }));
  }

  it.each(["en", "zh"] as const)("shows the picker's folder access limitation in %s without starting a transfer", async language => {
    setLocale(language);
    await clickRemoteFile(() => { throw '__rssh_err__|{"code":"sftp_folder_picker_unavailable","params":{}}'; });
    await vi.waitFor(() => expect(target.querySelector(".error-banner")?.textContent)
      .toBe(t("error.sftp_folder_picker_unavailable")));
    expect(transfers.list()).toHaveLength(0);
    expect(invokeMock.mock.calls.some(([command]) => command === "sftp_download_to")).toBe(false);
  });

  it("shows the folder grant instructions in the queue when a selected document grants no sibling access", async () => {
    const localPath = "content://provider/document/report";
    await clickRemoteFile(() => localPath);
    await vi.waitFor(() => expect(transfers.list()[0]?.status).toBe("failed"));
    expect(invokeMock).toHaveBeenCalledWith("sftp_download_to", expect.objectContaining({
      originalSessionId: "ssh", remotePath: "/report.txt", localPath,
    }));

    await unmount(component!);
    component = mount(DownloadsScreen, { target });
    for (const language of ["en", "zh"] as const) {
      setLocale(language);
      await tick();
      expect(target.querySelector(".error")?.textContent).toBe(t("error.sftp_provider_siblings_unavailable"));
      expect(target.querySelector(".row-actions")?.textContent).not.toContain(t("downloads.retry"));
    }
  });
});
