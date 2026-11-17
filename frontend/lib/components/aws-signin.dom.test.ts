import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import DynamicDiscoverySourceForm from "./DynamicDiscoverySourceForm.svelte";
import { setLocale } from "../i18n/index.svelte.ts";
import type { DynamicDiscoverySource } from "../stores/app.svelte.ts";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
let component: ReturnType<typeof mount> | undefined;
let source: DynamicDiscoverySource;
const save = vi.fn();
const login = { login_id: "attempt", authorization_url: "https://us-east-1.signin.aws.amazon.com/authorize", expires_in: 600 };
async function settle() { for (let n = 0; n < 4; n++) { flushSync(); await tick(); } }
function button(label: string) {
  return [...document.querySelectorAll<HTMLButtonElement>("button")].find(button => button.textContent?.trim() === label)!;
}

beforeEach(() => {
  setLocale("en");
  save.mockReset();
  source = { id: crypto.randomUUID(), name: "Mobile AWS", enabled: true, platform: "aws_native", region: "us-east-1", only_ssm_available: false };
  invoke.mockReset().mockImplementation(async command => {
    if (command === "aws_signin_status") return { authenticated: false };
    if (command === "aws_signin_start") return login;
    if (command === "aws_signin_complete") return { authenticated: true, account_id: "123456789012", arn: "arn:aws:iam::123456789012:user/test" };
    return null;
  });
});

async function render() {
  component = mount(DynamicDiscoverySourceForm, { target: document.body, props: {
    source, contexts: [], statuses: {}, availablePlatforms: ["aws_native"],
    onRefreshContexts: vi.fn(), onSave: save, onCancel: vi.fn(),
  } });
  await settle();
}

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.innerHTML = "";
});

it("offers browser authorization on cloud-only hosts and never saves the pasted code", async () => {
  await render();
  expect(document.body.textContent).not.toContain("Docker CLI");
  expect(document.body.textContent).not.toContain("session-manager-plugin");
  button("Sign in to AWS").click();
  await settle();
  expect(invoke).toHaveBeenCalledWith("aws_signin_start", { sourceId: source.id, region: "us-east-1" });
  expect(invoke).toHaveBeenCalledWith("open_external_url", { url: login.authorization_url });
  const code = document.querySelector<HTMLInputElement>('.code-field input')!;
  code.value = "copied-code";
  code.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  button("Complete sign-in").click();
  await settle();
  expect(invoke).toHaveBeenCalledWith("aws_signin_complete", { sourceId: source.id, loginId: "attempt", code: "copied-code" });
  expect(document.body.textContent).toContain("Ready to save · Account 123456789012");
  expect(document.querySelector('.code-field')).toBeNull();
  button("Save").click();
  expect(save).toHaveBeenCalledWith(source);
  expect(JSON.stringify(save.mock.calls)).not.toContain("copied-code");
});

it("cancels pending authorization when the source form closes", async () => {
  await render();
  button("Sign in to AWS").click();
  await settle();
  await unmount(component!);
  component = undefined;
  expect(invoke).toHaveBeenCalledWith("aws_signin_cancel", { sourceId: source.id, loginId: "attempt" });
});

it("invalidates pending authorization when its Region changes", async () => {
  await render();
  button("Sign in to AWS").click();
  await settle();
  const region = [...document.querySelectorAll<HTMLLabelElement>("label")].find(label => label.textContent?.trim() === "Region")!.querySelector<HTMLInputElement>("input")!;
  region.value = "eu-west-1";
  region.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  expect(invoke).toHaveBeenCalledWith("aws_signin_cancel", { sourceId: source.id, loginId: "attempt" });
  expect(document.querySelector('.code-field')).toBeNull();
});

it("keeps an in-flight identity read when an existing source's Region changes", async () => {
  let finishStatus!: (value: { authenticated: boolean; account_id: string }) => void;
  invoke.mockImplementationOnce(() => new Promise(resolve => { finishStatus = resolve; }));
  await render();
  expect(invoke).toHaveBeenCalledWith("aws_signin_status", { sourceId: source.id });
  const region = [...document.querySelectorAll<HTMLLabelElement>("label")].find(label => label.textContent?.trim() === "Region")!.querySelector<HTMLInputElement>("input")!;
  region.value = "eu-west-1";
  region.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  finishStatus({ authenticated: true, account_id: "123456789012" });
  await settle();
  expect(document.body.textContent).toContain("Signed in · Account 123456789012");
  expect(invoke.mock.calls.filter(([command]) => command === "aws_signin_status")).toHaveLength(1);
  expect(invoke).not.toHaveBeenCalledWith("aws_signin_cancel", expect.anything());
  expect(button("Save").disabled).toBe(false);
});

it("cancels authorization started in the previous Region before its URL arrives", async () => {
  await render();
  let finishStart!: (value: typeof login) => void;
  invoke.mockImplementationOnce(() => new Promise(resolve => { finishStart = resolve; }));
  button("Sign in to AWS").click();
  await settle();
  const region = [...document.querySelectorAll<HTMLLabelElement>("label")].find(label => label.textContent?.trim() === "Region")!.querySelector<HTMLInputElement>("input")!;
  region.value = "eu-west-1";
  region.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  finishStart(login);
  await settle();
  expect(invoke).toHaveBeenCalledWith("aws_signin_cancel", { sourceId: source.id, loginId: "attempt" });
  expect(invoke).not.toHaveBeenCalledWith("open_external_url", expect.anything());
  expect(document.querySelector('.code-field')).toBeNull();
});

it("shows authorization errors in the current locale instead of the wire payload", async () => {
  await render();
  invoke.mockRejectedValueOnce('__rssh_err__|{"code":"aws_auth_access_denied","params":{}}');
  button("Sign in to AWS").click();
  await settle();
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("AWS authorization was denied");
  expect(document.querySelectorAll(".form-error")).toHaveLength(1);
  expect(document.querySelector(".signin-error")).toBeNull();
  expect(document.body.textContent).not.toContain("__rssh_err__");
  setLocale("zh");
  await settle();
  expect(document.querySelector('[role="alert"]')?.textContent).toContain("AWS 授权被拒绝");
});

it("uses the parent form error once when code exchange fails during a challenge", async () => {
  await render();
  button("Sign in to AWS").click();
  await settle();
  const code = document.querySelector<HTMLInputElement>(".code-field input")!;
  code.value = "invalid-code";
  code.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  invoke.mockRejectedValueOnce('__rssh_err__|{"code":"aws_auth_access_denied","params":{}}');
  button("Complete sign-in").click();
  await settle();
  expect(document.querySelectorAll('[role="alert"]')).toHaveLength(1);
  expect(document.querySelector(".form-error")?.textContent).toContain("AWS authorization was denied");
  expect(document.body.textContent).not.toContain("Complete or cancel AWS sign-in before saving.");
  expect(document.querySelector(".signin-error")).toBeNull();
});
