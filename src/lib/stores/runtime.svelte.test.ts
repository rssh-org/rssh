import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

const desktop = {
  localPty: true,
  serial: true,
  localDiscovery: true,
  sshAgent: true,
  defaultKeyFiles: true,
  cliInstall: true,
  multiWindow: true,
  windowControls: true,
  windowPin: true,
  fileMultiSelect: true,
  directoryTransfer: true,
  plugins: true,
};

beforeEach(() => {
  vi.resetModules();
  invokeMock.mockReset();
});
afterEach(() => { vi.unstubAllGlobals(); });

describe("runtime capabilities", () => {
  it("does not enable native features until the backend reports support", async () => {
    const runtime = await import("./runtime.svelte.ts");
    expect(runtime.loaded()).toBe(false);
    expect(Object.values(runtime.capabilities()).every((supported) => supported === false)).toBe(true);
    invokeMock.mockResolvedValue({ ...desktop, localPty: false, multiWindow: false, serial: false });
    await runtime.load();
    expect(invokeMock).toHaveBeenCalledWith("get_runtime_capabilities");
    expect(runtime.loaded()).toBe(true);
    expect(runtime.capabilities().localPty).toBe(false);
    expect(runtime.capabilities().multiWindow).toBe(false);
    expect(runtime.capabilities().fileMultiSelect).toBe(true);
  });

  it("shares initialization and caches a successful result", async () => {
    const runtime = await import("./runtime.svelte.ts");
    let complete!: (value: typeof desktop) => void;
    invokeMock.mockReturnValue(new Promise((resolve) => { complete = resolve; }));
    const first = runtime.load();
    const second = runtime.load();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(runtime.loaded()).toBe(false);
    complete(desktop);
    await Promise.all([first, second]);
    await runtime.load();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(runtime.capabilities()).toEqual(desktop);
  });

  it("keeps failed initialization retryable without claiming unsupported features", async () => {
    const runtime = await import("./runtime.svelte.ts");
    invokeMock.mockRejectedValueOnce(new Error("backend unavailable"));
    await expect(runtime.load()).rejects.toThrow("backend unavailable");
    expect(runtime.loaded()).toBe(false);
    invokeMock.mockResolvedValue(desktop);
    await runtime.load();
    expect(runtime.loaded()).toBe(true);
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it("refreshes an IDE picker injected after startup without closing the resource barrier", async () => {
    const host = Object.assign(new EventTarget(), { __RSSH_IPC_SHIM__: true });
    vi.stubGlobal("window", host);
    const runtime = await import("./runtime.svelte.ts");
    invokeMock.mockResolvedValueOnce({ ...desktop, fileMultiSelect: false, directoryTransfer: false });
    await runtime.load();
    expect(runtime.capabilities().directoryTransfer).toBe(false);

    let finish!: (value: typeof desktop) => void;
    invokeMock.mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    host.dispatchEvent(new Event("rssh:host-ready"));
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(2));
    expect(runtime.loaded()).toBe(true);
    finish(desktop);
    await vi.waitFor(() => expect(runtime.capabilities().directoryTransfer).toBe(true));
    expect(runtime.loaded()).toBe(true);
  });

  it("does not lose host-ready delivered during the first capability request", async () => {
    const host = Object.assign(new EventTarget(), { __RSSH_IPC_SHIM__: true });
    vi.stubGlobal("window", host);
    const runtime = await import("./runtime.svelte.ts");
    let finish!: (value: typeof desktop) => void;
    invokeMock.mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    invokeMock.mockResolvedValueOnce(desktop);
    const initialization = runtime.load();
    host.dispatchEvent(new Event("rssh:host-ready"));
    finish({ ...desktop, fileMultiSelect: false, directoryTransfer: false });
    await initialization;
    await vi.waitFor(() => expect(runtime.capabilities().directoryTransfer).toBe(true));
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it("leaves a failed startup to the App retry flow when the IDE host becomes ready", async () => {
    const host = Object.assign(new EventTarget(), { __RSSH_IPC_SHIM__: true });
    vi.stubGlobal("window", host);
    const runtime = await import("./runtime.svelte.ts");
    invokeMock.mockRejectedValueOnce(new Error("backend unavailable"));
    await expect(runtime.load()).rejects.toThrow("backend unavailable");

    invokeMock.mockResolvedValue(desktop);
    host.dispatchEvent(new Event("rssh:host-ready"));
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(runtime.loaded()).toBe(false);

    await runtime.load();
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(runtime.loaded()).toBe(true);
  });
});
