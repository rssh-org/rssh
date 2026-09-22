import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
const subscribeMock = vi.hoisted(() => vi.fn());
vi.mock("../ipc-shim.ts", () => ({ onRuntimeCapabilitiesChanged: subscribeMock }));

function notifyCapabilitiesChanged(): void {
  subscribeMock.mock.calls[0][0]();
}

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
  nativeClipboard: true,
  terminalPolicy: {
    imageStorageLimitMb: 128,
    imagePixelLimit: 16_000_000,
    outputBacklogLimitBytes: 128 * 1024 * 1024,
    gpuRenderDefault: true,
  },
};

beforeEach(() => {
  vi.resetModules();
  invokeMock.mockReset();
  subscribeMock.mockReset();
});
afterEach(() => { vi.unstubAllGlobals(); });

describe("runtime capabilities", () => {
  it("keeps host terminal policy separate from feature capabilities", async () => {
    const runtime = await import("./runtime.svelte.ts");
    expect(() => runtime.terminalPolicy()).toThrow();
    invokeMock.mockResolvedValue(desktop);
    await runtime.load();
    expect(runtime.terminalPolicy()).toEqual(desktop.terminalPolicy);
    expect(runtime.capabilities()).not.toHaveProperty("terminalPolicy");
    expect(runtime.capabilities().nativeClipboard).toBe(true);
  });

  it("uses the backend's resource limits without inferring a device from input or width", async () => {
    vi.stubGlobal("window", { innerWidth: 1600 });
    vi.stubGlobal("navigator", { maxTouchPoints: 0, userAgent: "unrecognized host" });
    const runtime = await import("./runtime.svelte.ts");
    const terminalPolicy = {
      imageStorageLimitMb: 32,
      imagePixelLimit: 4_000_000,
      outputBacklogLimitBytes: 32 * 1024 * 1024,
      gpuRenderDefault: false,
    };
    invokeMock.mockResolvedValue({ ...desktop, terminalPolicy });
    await runtime.load();
    expect(runtime.terminalPolicy()).toEqual(terminalPolicy);
  });

  it("keeps the resource barrier closed if the host omits its terminal policy", async () => {
    const runtime = await import("./runtime.svelte.ts");
    const { terminalPolicy: _policy, ...capabilities } = desktop;
    invokeMock.mockResolvedValue(capabilities);
    await expect(runtime.load()).rejects.toThrow("terminal runtime policy");
    expect(runtime.loaded()).toBe(false);
  });

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
    const { terminalPolicy: _policy, ...capabilities } = desktop;
    expect(runtime.capabilities()).toEqual(capabilities);
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

  it("refreshes changed capabilities without closing the resource barrier", async () => {
    const runtime = await import("./runtime.svelte.ts");
    invokeMock.mockResolvedValueOnce({ ...desktop, fileMultiSelect: false, directoryTransfer: false });
    await runtime.load();
    expect(runtime.capabilities().directoryTransfer).toBe(false);

    let finish!: (value: typeof desktop) => void;
    invokeMock.mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    notifyCapabilitiesChanged();
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledTimes(2));
    expect(runtime.loaded()).toBe(true);
    finish(desktop);
    await vi.waitFor(() => expect(runtime.capabilities().directoryTransfer).toBe(true));
    expect(runtime.loaded()).toBe(true);
  });

  it("does not lose a change notification delivered during the first capability request", async () => {
    const runtime = await import("./runtime.svelte.ts");
    let finish!: (value: typeof desktop) => void;
    invokeMock.mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
    invokeMock.mockResolvedValueOnce(desktop);
    const initialization = runtime.load();
    notifyCapabilitiesChanged();
    finish({ ...desktop, fileMultiSelect: false, directoryTransfer: false });
    await initialization;
    await vi.waitFor(() => expect(runtime.capabilities().directoryTransfer).toBe(true));
    expect(invokeMock).toHaveBeenCalledTimes(2);
  });

  it("leaves a failed startup to the App retry flow when capabilities change", async () => {
    const runtime = await import("./runtime.svelte.ts");
    invokeMock.mockRejectedValueOnce(new Error("backend unavailable"));
    await expect(runtime.load()).rejects.toThrow("backend unavailable");

    invokeMock.mockResolvedValue(desktop);
    notifyCapabilitiesChanged();
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(runtime.loaded()).toBe(false);

    await runtime.load();
    expect(invokeMock).toHaveBeenCalledTimes(2);
    expect(runtime.loaded()).toBe(true);
  });
});
