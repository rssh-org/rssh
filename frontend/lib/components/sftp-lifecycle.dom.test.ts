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
