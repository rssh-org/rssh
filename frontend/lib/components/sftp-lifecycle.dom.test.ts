import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { mount, tick, unmount } from "svelte";
import DownloadsScreen from "./DownloadsScreen.svelte";
import * as app from "../stores/app.svelte.ts";
import * as transfers from "../stores/transfers.svelte.ts";
import type { SavedDownload } from "../file-access.ts";
import { t } from "../i18n/index.svelte.ts";
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


const savedReport: SavedDownload = {
  name: "report-copy.txt", localPath: "/app/Downloads/report-copy.txt", identity: "original", size: 12, isBackup: false,
};

it("uses an editable app download name and offers export after the transfer commits", async () => {
  vi.spyOn(app, "capabilities").mockReturnValue({ ...app.capabilities(), sftpDownloadExport: true });
  invokeMock.mockImplementation(async (command: string) => {
    if (command === "sftp_connect_session") return "sftp";
    if (command === "sftp_home") return "/";
    if (command === "sftp_list") return [{ name: "report.txt", size: 12, is_dir: false }];
    if (command === "sftp_pick_save_path") return "/app/Downloads/report-copy.txt";
    if (command === "sftp_download_to") return 12;
    if (command === "sftp_list_downloads") return [savedReport];
    return null;
  });
  component = mount(SftpBrowser, { target, props: { meta: { tabId: "tab", sessionId: "ssh" } } });
  await vi.waitFor(() => expect(target.querySelector(".file-name")).not.toBeNull());
  target.querySelector<HTMLButtonElement>(".file-name")!.click();
  await tick();
  const dialog = target.querySelector('[role="dialog"]')!;
  expect(dialog.textContent).toContain(t("sftp.export_after_download"));
  expect(invokeMock.mock.calls.some(([command]) => command === "sftp_download_to")).toBe(false);
  const input = dialog.querySelector("input")!;
  expect(input.value).toBe("report.txt");
  input.value = "report-copy.txt";
  input.dispatchEvent(new Event("input", { bubbles: true }));
  await tick();
  dialog.querySelectorAll("button")[1].click();
  await vi.waitFor(() => expect(transfers.list()[0]?.status).toBe("done"));
  expect(invokeMock).toHaveBeenCalledWith("sftp_pick_save_path", { defaultName: "report-copy.txt" });
  expect(invokeMock).toHaveBeenCalledWith("sftp_download_to", expect.objectContaining({
    originalSessionId: "ssh", remotePath: "/report.txt", localPath: "/app/Downloads/report-copy.txt",
  }));

  await unmount(component);
  component = mount(DownloadsScreen, { target });
  await vi.waitFor(() => expect(target.querySelector(".filename")?.textContent).toBe(savedReport.name));
  expect(target.querySelector(".row-actions")?.textContent).not.toContain(t("downloads.export"));
  const clear = [...target.querySelectorAll("button")].find(button => button.textContent?.trim() === t("downloads.clear_finished"));
  clear!.click();
  await tick();
  expect(transfers.list()).toHaveLength(0);
  const exportButton = [...target.querySelectorAll("button")].find(button => button.textContent?.trim() === t("downloads.export"));
  exportButton!.click();
  expect(invokeMock).toHaveBeenCalledWith("sftp_export_download", { localPath: savedReport.localPath, identity: "original" });
});

it("recovers saved files and backups from disk with an empty queue and requires a new selection after replacement", async () => {
  vi.spyOn(app, "capabilities").mockReturnValue({ ...app.capabilities(), sftpDownloadExport: true });
  const backup: SavedDownload = {
    name: ".rssh-download-saved.bak", localPath: "/app/Downloads/.rssh-download-saved.bak",
    identity: "backup", size: 29, isBackup: true,
  };
  let current = savedReport;
  const exportCalls: Array<{ localPath: string; identity: string }> = [];
  invokeMock.mockImplementation(async (cmd: string, args: { localPath: string; identity: string }) => {
    if (cmd === "sftp_list_downloads") return [current, backup];
    if (cmd === "sftp_export_download") {
      exportCalls.push(args);
      if (args.localPath === current.localPath && args.identity !== current.identity) {
        throw '__rssh_err__|{"code":"sftp_target_changed","params":{"path":"report-copy.txt"}}';
      }
    }
  });
  expect(transfers.list()).toHaveLength(0);
  component = mount(DownloadsScreen, { target });
  const button = (label: string) => [...target.querySelectorAll("section button")].find(b => b.textContent?.trim() === label) as HTMLButtonElement;
  await vi.waitFor(() => expect(target.querySelectorAll(".filename")).toHaveLength(2));
  button(t("downloads.export_backup")).click();
  expect(exportCalls[0]).toEqual({ localPath: backup.localPath, identity: backup.identity });

  current = { ...savedReport, identity: "replacement", size: 36 };
  button(t("downloads.export")).click();
  await vi.waitFor(() => expect(target.querySelector(".size")?.textContent).toBe("36 B"));
  expect(exportCalls).toHaveLength(2);
  expect(exportCalls[1].identity).toBe("original");
  button(t("downloads.export")).click();
  expect(exportCalls[2].identity).toBe("replacement");
});

it("refreshes the open file library after a download publishes", async () => {
  vi.spyOn(app, "capabilities").mockReturnValue({ ...app.capabilities(), sftpDownloadExport: true });
  let committed = false;
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "sftp_list_downloads") return committed ? [savedReport] : [];
    if (cmd === "sftp_connect_session") return "sftp";
    if (cmd === "sftp_download_to") { committed = true; return savedReport.size; }
  });
  component = mount(DownloadsScreen, { target });
  await vi.waitFor(() => expect(target.textContent).toContain(t("downloads.saved_empty")));
  await transfers.startDownload({ sessionId: "ssh", localPath: savedReport.localPath, remotePath: "/report" });
  await vi.waitFor(() => expect(target.querySelector(".filename")?.textContent).toBe(savedReport.name));
});
