import { invoke } from "@tauri-apps/api/core";
import { loadSettings } from "./store.svelte.ts";

export interface CodexDeviceLogin {
  login_id: string;
  verification_url: string;
  user_code: string;
  interval: number;
  expires_in: number;
}

/** One settings form owns one cancellable device login. Tokens stay in Rust. */
export function createCodexLogin() {
  let authenticated = $state(false);
  let challenge = $state<CodexDeviceLogin | null>(null);
  let busy = $state(false);
  let error = $state<unknown>(null);
  let generation = 0;
  let providerId = "";
  let timer: ReturnType<typeof setTimeout> | undefined;

  function invalidate() {
    generation += 1;
    clearTimeout(timer);
    timer = undefined;
    const previous = challenge;
    challenge = null;
    busy = false;
    return previous;
  }

  async function cancel() {
    const id = providerId;
    const previous = invalidate();
    const current = generation;
    if (!previous) return;
    try {
      await invoke("ai_codex_login_cancel", { providerId: id, loginId: previous.login_id });
    } catch (e) {
      if (current === generation && error === null) error = e;
    }
  }

  async function load(id: string) {
    const cleanup = cancel();
    const current = generation;
    await cleanup;
    if (current !== generation) return;
    providerId = id;
    authenticated = false;
    error = null;
    busy = true;
    try {
      const status = await invoke<{ authenticated: boolean }>("ai_codex_auth_status", { providerId: id });
      if (current === generation) authenticated = status.authenticated;
    } catch (e) {
      if (current === generation) error = e;
    } finally {
      if (current === generation) busy = false;
    }
  }

  function schedulePoll(id: string, login: CodexDeviceLogin, current: number, interval: number) {
    timer = setTimeout(async () => {
      timer = undefined;
      try {
        const result = await invoke<{ status: "pending" | "complete"; interval: number }>(
          "ai_codex_login_poll", { providerId: id, loginId: login.login_id },
        );
        if (current !== generation) return;
        if (result.status === "complete") {
          challenge = null;
          authenticated = true;
          await loadSettings();
        } else {
          schedulePoll(id, login, current, result.interval);
        }
      } catch (e) {
        if (current !== generation) return;
        error = e;
        // A failed poll ends this attempt; the user can explicitly try again.
        await cancel();
      }
    }, Math.max(1, interval) * 1000);
  }

  async function start(id: string) {
    const cleanup = cancel();
    const current = generation;
    await cleanup;
    if (current !== generation) return;
    providerId = id;
    busy = true;
    error = null;
    try {
      const login = await invoke<CodexDeviceLogin>("ai_codex_login_start", { providerId: id });
      if (current !== generation) {
        // Closing the form while start is in flight must cancel its late reply.
        await invoke("ai_codex_login_cancel", { providerId: id, loginId: login.login_id });
        return;
      }
      challenge = login;
      schedulePoll(id, login, current, login.interval);
    } catch (e) {
      if (current === generation) error = e;
    } finally {
      if (current === generation) busy = false;
    }
  }

  async function logout(id: string) {
    const cleanup = cancel();
    const current = generation;
    await cleanup;
    if (current !== generation) return;
    busy = true;
    error = null;
    try {
      await invoke("ai_codex_logout", { providerId: id });
      if (current !== generation) return;
      authenticated = false;
      await loadSettings();
    } catch (e) {
      if (current === generation) error = e;
    } finally {
      if (current === generation) busy = false;
    }
  }

  return {
    authenticated: () => authenticated,
    challenge: () => challenge,
    busy: () => busy,
    error: () => error,
    load,
    start,
    cancel,
    logout,
    dispose() {
      // Cleanup has no mounted UI to receive errors. Late starts are cancelled
      // by their generation check, and backend attempts also expire.
      void cancel();
    },
  };
}
