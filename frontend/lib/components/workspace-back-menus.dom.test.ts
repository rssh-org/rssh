import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import SftpBrowser from "./SftpBrowser.svelte";
import BlockContextMenu from "./BlockContextMenu.svelte";
import * as app from "../stores/app.svelte.ts";
import { registerBackHandler, requestBack } from "../stores/back.svelte.ts";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

let component: ReturnType<typeof mount> | undefined;
let removePage: (() => void) | undefined;
let target: HTMLDivElement;
const closePanel = vi.fn();

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

beforeEach(() => {
  closePanel.mockClear();
  invokeMock.mockReset().mockImplementation(async (command: string) => {
    if (command === "sftp_connect_session") return "sftp:back-test";
    if (command === "sftp_home") return "/home/test";
    if (command === "sftp_list") return [{ name: "example.txt", is_dir: false, size: 10, mtime: 0 }];
    return null;
  });
  target = document.createElement("div");
  document.body.append(target);
  removePage = registerBackHandler({ handle: closePanel, priority: 0 });
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  removePage?.();
  removePage = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

describe("workspace menus consume back before their page", () => {
  it("closes the SFTP file menu before hiding the SFTP panel", async () => {
    component = mount(SftpBrowser, { target, props: { meta: { sessionId: "ssh:back-test" } } });
    await vi.waitFor(() => expect(target.querySelector(".file-name")).not.toBeNull());
    target.querySelector(".file-name")!.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true }));
    await settle();
    expect(target.querySelector(".ctx-menu")).not.toBeNull();

    expect(requestBack()).toBe(true);
    await settle();
    expect(target.querySelector(".ctx-menu")).toBeNull();
    expect(closePanel).not.toHaveBeenCalled();
    expect(invokeMock.mock.calls.some(([command]) => command === "sftp_close")).toBe(false);

    expect(requestBack()).toBe(true);
    expect(closePanel).toHaveBeenCalledOnce();
  });

  it("closes the SFTP upload menu before hiding the SFTP panel", async () => {
    vi.spyOn(app, "capabilities").mockReturnValue({ ...app.capabilities(), directoryTransfer: true });
    component = mount(SftpBrowser, { target, props: { meta: { sessionId: "ssh:back-test" } } });
    await vi.waitFor(() => expect(target.querySelector(".file-name")).not.toBeNull());
    target.querySelector<HTMLButtonElement>('[aria-haspopup="menu"]')!.click();
    await settle();
    expect(target.querySelector(".upload-menu")).not.toBeNull();

    expect(requestBack()).toBe(true);
    await settle();
    expect(target.querySelector(".upload-menu")).toBeNull();
    expect(closePanel).not.toHaveBeenCalled();

    expect(requestBack()).toBe(true);
    expect(closePanel).toHaveBeenCalledOnce();
  });

  it("closes the terminal block menu without performing its action or navigating", async () => {
    const closeMenu = vi.fn();
    const action = vi.fn();
    component = mount(BlockContextMenu, {
      target, props: { x: 0, y: 0, items: [{ label: "Collapse", action }], onClose: closeMenu },
    });
    await settle();

    expect(requestBack()).toBe(true);
    expect(closeMenu).toHaveBeenCalledOnce();
    expect(action).not.toHaveBeenCalled();
    expect(closePanel).not.toHaveBeenCalled();
  });
});
