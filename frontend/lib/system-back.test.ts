import { beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock, listenMock } = vi.hoisted(() => ({ invokeMock: vi.fn(), listenMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));

beforeEach(() => { vi.resetModules(); invokeMock.mockReset().mockResolvedValue(null); listenMock.mockReset(); });

describe("system back bridge", () => {
  it("subscribes to the owning window before enabling native back", async () => {
    const { connectSystemBack } = await import("./system-back.ts");
    let finish!: (value: () => void) => void;
    listenMock.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const handle = vi.fn();
    const bridge = connectSystemBack("main", handle, vi.fn());
    bridge.setEnabled(true);
    expect(invokeMock).not.toHaveBeenCalled();
    expect(listenMock).toHaveBeenCalledWith("navigation:back:main", expect.any(Function));
    finish(vi.fn());
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith("set_back_navigation_enabled", { enabled: true }));
    listenMock.mock.calls[0][1]({ payload: null });
    expect(handle).toHaveBeenCalledOnce();
    bridge.dispose();
  });

  it("never enables native interception if disposed while subscription is pending", async () => {
    const { connectSystemBack } = await import("./system-back.ts");
    let finish!: (value: () => void) => void;
    listenMock.mockReturnValue(new Promise((resolve) => { finish = resolve; }));
    const stop = vi.fn();
    const handle = vi.fn();
    const bridge = connectSystemBack("main", handle, vi.fn());
    bridge.setEnabled(true);
    bridge.dispose();
    finish(stop);
    await vi.waitFor(() => expect(stop).toHaveBeenCalledOnce());
    expect(invokeMock.mock.calls).toEqual([["set_back_navigation_enabled", { enabled: false }]]);
    listenMock.mock.calls[0][1]({ payload: null });
    expect(handle).not.toHaveBeenCalled();
  });

  it("disables the native callback after an in-flight enable before unsubscribing", async () => {
    const { connectSystemBack } = await import("./system-back.ts");
    const stop = vi.fn();
    listenMock.mockResolvedValue(stop);
    let finish!: () => void;
    invokeMock.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    const bridge = connectSystemBack("main", vi.fn(), vi.fn());
    bridge.setEnabled(true);
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledOnce());
    bridge.dispose();
    expect(stop).not.toHaveBeenCalled();
    finish();
    await vi.waitFor(() => expect(stop).toHaveBeenCalledOnce());
    expect(invokeMock.mock.calls).toEqual([
      ["set_back_navigation_enabled", { enabled: true }],
      ["set_back_navigation_enabled", { enabled: false }],
    ]);
  });

  it("reports a subscription failure without enabling native back", async () => {
    const { connectSystemBack } = await import("./system-back.ts");
    const error = new Error("event listener unavailable");
    const onError = vi.fn();
    listenMock.mockRejectedValue(error);
    const bridge = connectSystemBack("main", vi.fn(), onError);
    bridge.setEnabled(true);
    await vi.waitFor(() => expect(onError).toHaveBeenCalledWith(error));
    expect(invokeMock).not.toHaveBeenCalled();
    bridge.dispose();
  });
});
