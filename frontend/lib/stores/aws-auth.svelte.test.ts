import { beforeEach, expect, it, vi } from "vitest";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
const login = { login_id: "login-1", authorization_url: "https://us-east-1.signin.aws.amazon.com/authorize", expires_in: 600 };

beforeEach(() => { vi.resetModules(); invoke.mockReset(); });

it("keeps AWS sign-in state scoped to each source and sends only a code to completion", async () => {
  const auth = await import("./aws-auth.svelte.ts");
  invoke.mockResolvedValueOnce(login);
  await auth.start("source-a", "us-east-1");
  expect(auth.challenge("source-a")).toEqual(login);
  expect(auth.challenge("source-b")).toBeNull();
  invoke.mockResolvedValueOnce({ authenticated: true, account_id: "123456789012", arn: "arn:aws:iam::123456789012:user/test" });
  expect(await auth.complete("source-a", "  copied-code  ")).toBe(true);
  expect(invoke).toHaveBeenLastCalledWith("aws_signin_complete", { sourceId: "source-a", loginId: "login-1", code: "copied-code" });
  expect(auth.status("source-a").authenticated).toBe(true);
  expect(auth.status("source-b").authenticated).toBe(false);
  expect(auth.challenge("source-a")).toBeNull();
  expect(auth.commitRequest("source-a")).toEqual({ source_id: "source-a", login_id: "login-1" });
});

it("discards a completed draft and restores the saved identity without signing it out", async () => {
  const auth = await import("./aws-auth.svelte.ts");
  const saved = { authenticated: true, account_id: "111111111111" };
  const draft = { authenticated: true, account_id: "222222222222" };
  invoke.mockResolvedValueOnce(saved);
  await auth.load("source");
  invoke.mockResolvedValueOnce(login);
  await auth.start("source", "us-east-1");
  invoke.mockResolvedValueOnce(draft);
  await auth.complete("source", "code");
  expect(auth.status("source")).toEqual(draft);
  invoke.mockClear();
  await auth.cancel("source");
  expect(invoke).toHaveBeenCalledExactlyOnceWith("aws_signin_cancel", { sourceId: "source", loginId: "login-1" });
  expect(auth.status("source")).toEqual(saved);
  expect(auth.commitRequest("source")).toBeUndefined();
});

it("keeps a committed identity when the form later cleans up", async () => {
  const auth = await import("./aws-auth.svelte.ts");
  invoke.mockResolvedValueOnce(login);
  await auth.start("source", "us-east-1");
  const saved = { authenticated: true, account_id: "123456789012" };
  invoke.mockResolvedValueOnce(saved);
  await auth.complete("source", "code");
  auth.markCommitted("source", "login-1");
  invoke.mockClear();
  await auth.cancel("source");
  expect(invoke).not.toHaveBeenCalled();
  expect(auth.status("source")).toEqual(saved);
  expect(auth.commitRequest("source")).toBeUndefined();
});

it("does not let a late save response commit a newer authorization draft", async () => {
  const auth = await import("./aws-auth.svelte.ts");
  const saved = { authenticated: true, account_id: "111111111111" };
  invoke.mockResolvedValueOnce(saved);
  await auth.load("source");
  invoke.mockResolvedValueOnce({ ...login, login_id: "newer-login" });
  await auth.start("source", "us-east-1");
  invoke.mockResolvedValueOnce({ authenticated: true, account_id: "222222222222" });
  await auth.complete("source", "new-code");
  auth.markCommitted("source", "older-login");
  expect(auth.commitRequest("source")).toEqual({ source_id: "source", login_id: "newer-login" });
  await auth.cancel("source");
  expect(auth.status("source")).toEqual(saved);
});

it("cancels a login that arrives after its source form was closed", async () => {
  const auth = await import("./aws-auth.svelte.ts");
  let finish!: (value: typeof login) => void;
  invoke.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const starting = auth.start("source", "us-east-1");
  await auth.cancel("source");
  finish(login);
  expect(await starting).toBeNull();
  expect(invoke).toHaveBeenLastCalledWith("aws_signin_cancel", { sourceId: "source", loginId: "login-1" });
  expect(auth.challenge("source")).toBeNull();
  expect(auth.busy("source")).toBe(false);
});

it("does not restore authenticated UI from a completion after cancellation", async () => {
  const auth = await import("./aws-auth.svelte.ts");
  invoke.mockResolvedValueOnce(login);
  await auth.start("source", "us-east-1");
  let finish!: (value: { authenticated: boolean }) => void;
  invoke.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const completing = auth.complete("source", "code");
  await auth.cancel("source");
  finish({ authenticated: true });
  expect(await completing).toBe(false);
  expect(auth.status("source").authenticated).toBe(false);
  expect(auth.challenge("source")).toBeNull();
});

it("retains the attempt on a failed code exchange and reports the localized wire error", async () => {
  const auth = await import("./aws-auth.svelte.ts");
  invoke.mockResolvedValueOnce(login);
  await auth.start("source", "us-east-1");
  const error = '__rssh_err__|{"code":"aws_login_code_invalid","params":{}}';
  invoke.mockRejectedValueOnce(error);
  expect(await auth.complete("source", "bad-code")).toBe(false);
  expect(auth.error("source")).toBe(error);
  expect(auth.challenge("source")).toEqual(login);
  expect(auth.busy("source")).toBe(false);
});

it("keeps the saved account identity when backend logout fails", async () => {
  const auth = await import("./aws-auth.svelte.ts");
  const saved = { authenticated: true, account_id: "123456789012" };
  invoke.mockResolvedValueOnce(saved);
  await auth.load("source");
  invoke.mockRejectedValueOnce(new Error("logout failed"));
  await expect(auth.logout("source")).rejects.toThrow("logout failed");
  expect(invoke).toHaveBeenLastCalledWith("aws_signin_logout", { sourceId: "source" });
  expect(auth.status("source")).toEqual(saved);
});
