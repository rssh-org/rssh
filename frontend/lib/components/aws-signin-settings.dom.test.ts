import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import DynamicDiscoverySettings from "./DynamicDiscoverySettings.svelte";
import * as auth from "../stores/aws-auth.svelte.ts";
import * as discovery from "../stores/discovery.svelte.ts";
import type { DynamicDiscoverySource } from "../stores/app.svelte.ts";
import { setLocale } from "../i18n/index.svelte.ts";

const invoke = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());
const capabilities = vi.hoisted(() => ({ localDiscovery: false, cloudDiscovery: true }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("../stores/runtime.svelte.ts", () => ({ capabilities: () => capabilities }));
vi.mock("../stores/toast.svelte.ts", () => ({ toast: { error: toastError, success: vi.fn() } }));

let component: ReturnType<typeof mount> | undefined;
let sources: DynamicDiscoverySource[];
let savedStatus: auth.AwsSigninStatus;
let failSave: boolean;
const sourcesChanged = '__rssh_err__|{"code":"dynamic_discovery_sources_changed","params":{}}';
const draftStatus = { authenticated: true, account_id: "222222222222", arn: "arn:aws:iam::222222222222:user/new" };
const login = { login_id: "draft-login", authorization_url: "https://us-east-1.signin.aws.amazon.com/authorize", expires_in: 600 };

async function settle() { for (let n = 0; n < 6; n++) { flushSync(); await tick(); } }
function button(label: string) {
  const found = [...document.querySelectorAll<HTMLButtonElement>("button")].find(entry => entry.textContent?.trim() === label);
  expect(found, `button ${label}`).toBeDefined();
  return found!;
}
async function render() {
  component = mount(DynamicDiscoverySettings, { target: document.body });
  await settle();
}
async function authorize(existing = false) {
  button(existing ? "Sign in again" : "Sign in to AWS").click();
  await settle();
  const code = document.querySelector<HTMLInputElement>(".code-field input")!;
  code.value = "copied-code";
  code.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  button("Complete sign-in").click();
  await settle();
  expect(document.body.textContent).toContain("Ready to save · Account 222222222222");
}
function existingSource(): DynamicDiscoverySource {
  return { id: crypto.randomUUID(), name: "Saved AWS", enabled: true, platform: "aws_native", region: "us-east-1", only_ssm_available: false };
}
async function editExisting() {
  const source = existingSource();
  sources = [source];
  savedStatus = { authenticated: true, account_id: "111111111111" };
  await render();
  document.querySelector<HTMLButtonElement>('[aria-label="Edit Saved AWS"]')!.click();
  await settle();
  return source;
}

beforeEach(() => {
  setLocale("en");
  sources = [];
  savedStatus = { authenticated: false };
  failSave = false;
  capabilities.localDiscovery = false;
  toastError.mockReset();
  invoke.mockReset().mockImplementation(async (command, args) => {
    if (command === "list_dynamic_discovery_sources") return structuredClone(sources);
    if (command === "aws_signin_status") return savedStatus;
    if (command === "aws_signin_start") return login;
    if (command === "aws_signin_complete") return draftStatus;
    if (command === "save_dynamic_discovery_sources") {
      if (failSave) throw new Error("Source save failed");
      if (JSON.stringify(args.expectedSources) !== JSON.stringify(sources)) throw sourcesChanged;
      // IPC serializes Svelte's source proxies before the backend receives them.
      sources = JSON.parse(JSON.stringify(args.sources));
      if (args.authCommit) savedStatus = draftStatus;
      return null;
    }
    if (command === "get_setting") return JSON.stringify({ auto_refresh: false, interval_seconds: 60 });
    if (command === "dynamic_discovery_tool_status") return { platform: args.platform, available: true, error: null };
    return [];
  });
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.innerHTML = "";
  vi.restoreAllMocks();
});

it("submits a completed new-source draft only when Save is clicked", async () => {
  await render();
  button("+ New Source").click();
  await settle();
  const region = [...document.querySelectorAll("label")].find(label => label.textContent?.trim() === "Region")!.querySelector("input")!;
  region.value = "us-east-1";
  region.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  await authorize();
  const sourceId = invoke.mock.calls.find(([command]) => command === "aws_signin_start")![1].sourceId;
  expect(sources).toEqual([]);
  expect(savedStatus.authenticated).toBe(false);
  expect(invoke.mock.calls.some(([command]) => command === "save_dynamic_discovery_sources")).toBe(false);
  expect(auth.commitRequest(sourceId)).toEqual({ source_id: sourceId, login_id: login.login_id });
  button("Save").click();
  await settle();
  expect(invoke).toHaveBeenCalledWith("save_dynamic_discovery_sources", {
    sources: [expect.objectContaining({ id: sourceId, platform: "aws_native", region: "us-east-1" })],
    authCommit: { source_id: sourceId, login_id: login.login_id },
    expectedSources: [],
  });
  expect(auth.commitRequest(sourceId)).toBeUndefined();
  expect(auth.status(sourceId)).toEqual(draftStatus);
  expect(invoke.mock.calls.some(([command]) => command === "aws_signin_cancel" || command === "aws_signin_logout")).toBe(false);
  expect(JSON.stringify(sources)).not.toContain("copied-code");
});

it("cancels an edited source's completed draft without logging out its saved identity", async () => {
  const source = await editExisting();
  await authorize(true);
  button("Cancel").click();
  await settle();
  expect(invoke).toHaveBeenCalledWith("aws_signin_cancel", { sourceId: source.id, loginId: login.login_id });
  expect(invoke.mock.calls.some(([command]) => command === "aws_signin_logout" || command === "save_dynamic_discovery_sources")).toBe(false);
  expect(auth.status(source.id)).toEqual(savedStatus);
  expect(savedStatus.account_id).toBe("111111111111");
  expect(auth.commitRequest(source.id)).toBeUndefined();
});

it("keeps an edited draft after a failed save and commits the same login on retry", async () => {
  const changed = vi.spyOn(discovery, "sourcesChanged");
  const source = await editExisting();
  await authorize(true);
  failSave = true;
  button("Save").click();
  await settle();
  expect(document.querySelectorAll(".form-error")).toHaveLength(1);
  expect(document.querySelector(".form-error")?.textContent).toContain("Source save failed");
  expect(toastError).not.toHaveBeenCalled();
  expect(savedStatus.account_id).toBe("111111111111");
  expect(auth.commitRequest(source.id)).toEqual({ source_id: source.id, login_id: login.login_id });
  expect(button("Save").disabled).toBe(false);
  failSave = false;
  button("Save").click();
  await settle();
  const saves = invoke.mock.calls.filter(([command]) => command === "save_dynamic_discovery_sources");
  expect(saves).toHaveLength(2);
  for (const [, args] of saves) expect(args.authCommit).toEqual({ source_id: source.id, login_id: login.login_id });
  expect(auth.commitRequest(source.id)).toBeUndefined();
  expect(savedStatus).toEqual(draftStatus);
  expect(changed).toHaveBeenCalledWith([source], [source], [source.id]);
  expect(invoke.mock.calls.some(([command]) => command === "aws_signin_cancel" || command === "aws_signin_logout")).toBe(false);
});

it("discards a completed draft when its Region changes", async () => {
  const source = await editExisting();
  await authorize(true);
  const region = [...document.querySelectorAll("label")].find(label => label.textContent?.trim() === "Region")!.querySelector("input")!;
  region.value = "eu-west-1";
  region.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  expect(invoke).toHaveBeenCalledWith("aws_signin_cancel", { sourceId: source.id, loginId: login.login_id });
  expect(auth.commitRequest(source.id)).toBeUndefined();
  expect(document.body.textContent).toContain("Signed in · Account 111111111111");
  expect(invoke.mock.calls.some(([command]) => command === "aws_signin_logout")).toBe(false);
});

it("discards the old form's draft when opening another source form", async () => {
  const source = await editExisting();
  await authorize(true);
  button("+ New Source").click();
  await settle();
  expect(invoke).toHaveBeenCalledWith("aws_signin_cancel", { sourceId: source.id, loginId: login.login_id });
  expect(auth.commitRequest(source.id)).toBeUndefined();
  expect(auth.status(source.id).account_id).toBe("111111111111");
  expect(invoke.mock.calls.some(([command]) => command === "aws_signin_logout")).toBe(false);
});

it("discards an edited draft on settings unmount and preserves saved credentials", async () => {
  const source = await editExisting();
  await authorize(true);
  await unmount(component!);
  component = undefined;
  await settle();
  expect(invoke).toHaveBeenCalledWith("aws_signin_cancel", { sourceId: source.id, loginId: login.login_id });
  expect(invoke.mock.calls.some(([command]) => command === "aws_signin_logout")).toBe(false);
  expect(auth.status(source.id).account_id).toBe("111111111111");
});

it("omits authCommit when saving a source without a new authorization", async () => {
  const source = await editExisting();
  button("Save").click();
  await settle();
  expect(invoke).toHaveBeenCalledWith("save_dynamic_discovery_sources", { sources: [source], expectedSources: [source] });
  expect(invoke.mock.calls.some(([command]) => command === "aws_signin_logout")).toBe(false);
});

it("discards a draft when switching away from native AWS and saves no authCommit", async () => {
  capabilities.localDiscovery = true;
  const source = await editExisting();
  await authorize(true);
  [...document.querySelectorAll<HTMLButtonElement>(".cli-card")].find(entry => entry.textContent?.includes("AWS SSM (CLI)"))!.click();
  await settle();
  expect(invoke).toHaveBeenCalledWith("aws_signin_cancel", { sourceId: source.id, loginId: login.login_id });
  expect(invoke.mock.calls.some(([command]) => command === "aws_signin_logout")).toBe(false);
  expect(auth.status(source.id).account_id).toBe("111111111111");
  button("Save").click();
  await settle();
  expect(invoke).toHaveBeenCalledWith("save_dynamic_discovery_sources", {
    sources: [expect.objectContaining({ id: source.id, platform: "aws_ssm" })],
    expectedSources: [source],
  });
});

it("keeps edited input and authorization through a conflict, then retries against the refreshed list", async () => {
  const source = await editExisting();
  await authorize(true);
  const name = document.querySelector<HTMLInputElement>('.inline-form input[type="text"]')!;
  name.value = "My pending source name";
  name.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  const otherSource = { ...existingSource(), name: "Added in another window" };
  sources = [{ ...source, name: "Changed in another window" }, otherSource];
  const refreshedSources = structuredClone(sources);
  button("Save").click();
  await settle();
  expect(document.querySelectorAll('[role="alert"]')).toHaveLength(1);
  expect(document.body.textContent).toContain("Sources were changed in another window");
  expect(button("Save").disabled).toBe(true);
  expect(toastError).not.toHaveBeenCalled();
  expect(auth.commitRequest(source.id)?.login_id).toBe(login.login_id);
  expect(savedStatus.account_id).toBe("111111111111");
  button("Refresh source list").click();
  await settle();
  expect(document.querySelector('.inline-form input[type="text"]')).toBe(name);
  expect(name.value).toBe("My pending source name");
  expect(document.body.textContent).toContain(otherSource.name);
  expect(document.body.textContent).toContain("Review your form before saving again");
  expect(button("Save").disabled).toBe(false);
  expect(auth.commitRequest(source.id)?.login_id).toBe(login.login_id);
  expect(invoke.mock.calls.filter(([command]) => command === "save_dynamic_discovery_sources")).toHaveLength(1);
  button("Save").click();
  await settle();
  const saves = invoke.mock.calls.filter(([command]) => command === "save_dynamic_discovery_sources");
  expect(saves).toHaveLength(2);
  expect(saves[0][1].expectedSources).toEqual([source]);
  expect(saves[1][1]).toEqual({
    sources: [{ ...source, name: "My pending source name" }, otherSource],
    authCommit: { source_id: source.id, login_id: login.login_id },
    expectedSources: refreshedSources,
  });
  expect(sources[1]).toEqual(otherSource);
  expect(auth.commitRequest(source.id)).toBeUndefined();
  expect(invoke.mock.calls.some(([command]) => command === "aws_signin_cancel" || command === "aws_signin_logout")).toBe(false);
});

it("preserves a draft when refresh reveals its source was deleted and blocks a no-op save", async () => {
  const source = await editExisting();
  await authorize(true);
  sources = [];
  button("Save").click();
  await settle();
  button("Refresh source list").click();
  await settle();
  expect(document.body.textContent).toContain("This source was deleted in another window");
  expect(document.body.textContent).toContain("Ready to save · Account 222222222222");
  expect(button("Save").disabled).toBe(true);
  expect(auth.commitRequest(source.id)?.login_id).toBe(login.login_id);
  expect(invoke.mock.calls.filter(([command]) => command === "save_dynamic_discovery_sources")).toHaveLength(1);
  expect(invoke.mock.calls.some(([command]) => command === "aws_signin_cancel" || command === "aws_signin_logout")).toBe(false);
  button("Cancel").click();
  await settle();
  expect(auth.commitRequest(source.id)).toBeUndefined();
  expect(invoke).toHaveBeenCalledWith("aws_signin_cancel", { sourceId: source.id, loginId: login.login_id });
  expect(invoke.mock.calls.some(([command]) => command === "aws_signin_logout")).toBe(false);
});

it("keeps a new source and its authorization after a failed conflict refresh, then preserves concurrent additions", async () => {
  await render();
  button("+ New Source").click();
  await settle();
  const region = [...document.querySelectorAll("label")].find(label => label.textContent?.trim() === "Region")!.querySelector("input")!;
  region.value = "us-east-1";
  region.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  await authorize();
  const sourceId = invoke.mock.calls.find(([command]) => command === "aws_signin_start")![1].sourceId;
  const concurrent = { ...existingSource(), name: "Concurrent addition" };
  sources = [concurrent];
  button("Save").click();
  await settle();
  invoke.mockRejectedValueOnce(new Error("Source list unavailable"));
  button("Refresh source list").click();
  await settle();
  expect(toastError).toHaveBeenCalledWith("Source list unavailable");
  expect(button("Save").disabled).toBe(true);
  expect(auth.commitRequest(sourceId)?.login_id).toBe(login.login_id);
  expect(region.value).toBe("us-east-1");
  button("Refresh source list").click();
  await settle();
  expect(button("Save").disabled).toBe(false);
  button("Save").click();
  await settle();
  const saves = invoke.mock.calls.filter(([command]) => command === "save_dynamic_discovery_sources");
  expect(saves).toHaveLength(2);
  expect(saves[0][1].expectedSources).toEqual([]);
  expect(saves[1][1].expectedSources).toEqual([concurrent]);
  expect(sources).toEqual([concurrent, expect.objectContaining({ id: sourceId, region: "us-east-1" })]);
  expect(auth.commitRequest(sourceId)).toBeUndefined();
  expect(invoke.mock.calls.some(([command]) => command === "aws_signin_cancel" || command === "aws_signin_logout")).toBe(false);
});

it.each(["toggle", "delete"])("sends the current source list when saving a %s and offers conflict recovery", async action => {
  const source = existingSource();
  sources = [source];
  await render();
  sources = [source, { ...existingSource(), name: "Concurrent addition" }];
  if (action === "toggle") document.querySelector<HTMLInputElement>('.item-actions input[type="checkbox"]')!.click();
  else document.querySelector<HTMLButtonElement>('[aria-label="Delete Saved AWS"]')!.click();
  await settle();
  expect(invoke).toHaveBeenCalledWith("save_dynamic_discovery_sources", {
    sources: action === "toggle" ? [{ ...source, enabled: false }] : [],
    expectedSources: [source],
  });
  expect(document.body.textContent).toContain("Sources were changed in another window");
  button("Refresh source list").click();
  await settle();
  expect(document.body.textContent).toContain("Concurrent addition");
  expect(document.querySelector<HTMLInputElement>('.item-actions input[type="checkbox"]')!.checked).toBe(true);
  expect(invoke.mock.calls.filter(([command]) => command === "save_dynamic_discovery_sources")).toHaveLength(1);
});
