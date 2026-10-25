import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createCodexLogin, type CodexDeviceLogin } from "./codex-login.svelte.ts";

const { invoke, loadSettings } = vi.hoisted(() => ({
  invoke: vi.fn(),
  loadSettings: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("./store.svelte.ts", () => ({ loadSettings }));

const challenge: CodexDeviceLogin = {
  login_id: "attempt-a",
  verification_url: "https://auth.openai.com/codex/device",
  user_code: "ABCD-EFGH",
  interval: 3,
  expires_in: 900,
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}

beforeEach(() => {
  vi.useFakeTimers();
  invoke.mockReset();
  loadSettings.mockReset().mockResolvedValue({});
  invoke.mockImplementation(async (command: string) => {
    if (command === "ai_codex_login_start") return challenge;
    if (command === "ai_codex_login_poll") return { status: "pending", interval: 5 };
    if (command === "ai_codex_auth_status") return { authenticated: false };
  });
});

afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
});

describe("Codex device login", () => {
  it("polls at the server interval and follows changed intervals without overlap", async () => {
    const login = createCodexLogin();
    await login.start("provider-a");
    expect(login.challenge()).toEqual(challenge);
    await vi.advanceTimersByTimeAsync(2999);
    expect(invoke).not.toHaveBeenCalledWith("ai_codex_login_poll", expect.anything());
    await vi.advanceTimersByTimeAsync(1);
    expect(invoke).toHaveBeenCalledWith("ai_codex_login_poll", { providerId: "provider-a", loginId: "attempt-a" });
    invoke.mockClear();
    await vi.advanceTimersByTimeAsync(4999);
    expect(invoke).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(invoke).toHaveBeenCalledTimes(1);
    login.dispose();
  });

  it("finishes authorization, refreshes AI credentials and stops polling", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "ai_codex_login_start") return challenge;
      if (command === "ai_codex_login_poll") return { status: "complete", interval: 3 };
    });
    const login = createCodexLogin();
    await login.start("provider-a");
    await vi.advanceTimersByTimeAsync(3000);
    expect(login.authenticated()).toBe(true);
    expect(login.challenge()).toBeNull();
    expect(loadSettings).toHaveBeenCalledTimes(1);
    invoke.mockClear();
    login.dispose();
    await vi.advanceTimersByTimeAsync(60000);
    // Closing a successful form must not undo the completed authorization.
    expect(invoke).not.toHaveBeenCalled();
  });

  it("cancels a start that replies after its form closes", async () => {
    const started = deferred<CodexDeviceLogin>();
    invoke.mockImplementation((command: string) => command === "ai_codex_login_start" ? started.promise : Promise.resolve());
    const login = createCodexLogin();
    const pending = login.start("provider-a");
    await Promise.resolve();
    login.dispose();
    started.resolve(challenge);
    await pending;
    expect(login.challenge()).toBeNull();
    expect(login.busy()).toBe(false);
    expect(invoke).toHaveBeenCalledWith("ai_codex_login_cancel", { providerId: "provider-a", loginId: "attempt-a" });
    expect(vi.getTimerCount()).toBe(0);
  });

  it("does not resurrect a start cancelled before its first request", async () => {
    const login = createCodexLogin();
    const pending = login.start("provider-a");
    login.dispose();
    await pending;
    expect(invoke).not.toHaveBeenCalled();
  });

  it("ignores an in-flight poll completing after cancellation", async () => {
    const polled = deferred<{ status: string; interval: number }>();
    invoke.mockImplementation(async (command: string) => {
      if (command === "ai_codex_login_start") return challenge;
      if (command === "ai_codex_login_poll") return polled.promise;
    });
    const login = createCodexLogin();
    await login.start("provider-a");
    await vi.advanceTimersByTimeAsync(3000);
    await login.cancel();
    polled.resolve({ status: "complete", interval: 3 });
    await vi.advanceTimersByTimeAsync(0);
    expect(login.authenticated()).toBe(false);
    expect(loadSettings).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it("shows poll failures and cancels the attempt instead of silently retrying", async () => {
    const failure = new Error("device authorization expired");
    invoke.mockImplementation(async (command: string) => {
      if (command === "ai_codex_login_start") return challenge;
      if (command === "ai_codex_login_poll") throw failure;
    });
    const login = createCodexLogin();
    await login.start("provider-a");
    await vi.advanceTimersByTimeAsync(3000);
    expect(login.error()).toBe(failure);
    expect(login.challenge()).toBeNull();
    expect(invoke).toHaveBeenCalledWith("ai_codex_login_cancel", { providerId: "provider-a", loginId: "attempt-a" });
    expect(vi.getTimerCount()).toBe(0);
  });

  it("does not let a stale account status replace a newer device login", async () => {
    const status = deferred<{ authenticated: boolean }>();
    invoke.mockImplementation(async (command: string) => {
      if (command === "ai_codex_auth_status") return status.promise;
      if (command === "ai_codex_login_start") return challenge;
    });
    const login = createCodexLogin();
    const pending = login.load("provider-a");
    await Promise.resolve();
    await login.start("provider-b");
    status.resolve({ authenticated: true });
    await pending;
    expect(login.authenticated()).toBe(false);
    expect(login.challenge()).toEqual(challenge);
    login.dispose();
  });

  it("clears the local signed-in status and refreshes settings after logout", async () => {
    invoke.mockImplementation(async (command: string) => {
      if (command === "ai_codex_auth_status") return { authenticated: true };
    });
    const login = createCodexLogin();
    await login.load("provider-a");
    expect(login.authenticated()).toBe(true);
    await login.logout("provider-a");
    expect(login.authenticated()).toBe(false);
    expect(invoke).toHaveBeenCalledWith("ai_codex_logout", { providerId: "provider-a" });
    expect(loadSettings).toHaveBeenCalledTimes(1);
  });
});
