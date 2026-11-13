import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import DynamicDiscoverySourceForm from "./DynamicDiscoverySourceForm.svelte";
import { setLocale } from "../i18n/index.svelte.ts";
import type { DynamicDiscoverySource } from "../stores/app.svelte.ts";

let component: ReturnType<typeof mount> | undefined;
const save = vi.fn();
const source: DynamicDiscoverySource = {
  id: "aws", name: "AWS SSM", enabled: true, platform: "aws_ssm", profile: "", region: "us-east-1", only_ssm_available: false,
};
async function settle() { flushSync(); await tick(); flushSync(); }
function button(text: string) {
  return [...document.querySelectorAll<HTMLButtonElement>("button")].find((entry) => entry.textContent?.trim() === text)!;
}
function input(label: string) {
  return [...document.querySelectorAll<HTMLLabelElement>("label")].find((entry) => entry.textContent?.includes(label))!.querySelector<HTMLInputElement>("input")!;
}
async function render(value: DynamicDiscoverySource = source, available = true) {
  component = mount(DynamicDiscoverySourceForm, { target: document.body, props: {
    source: value, contexts: [{ platform: "aws_ssm", name: "production", current: true }],
    statuses: { [value.platform]: { platform: value.platform, available, version: "2", error: null } },
    onRefreshContexts: vi.fn(), onSave: save, onCancel: vi.fn(),
  } });
  await settle();
}
beforeEach(() => { setLocale("en"); save.mockReset(); });
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.innerHTML = "";
});

it("keeps the default AWS credential chain and saves the optional online-agent filter", async () => {
  await render();
  expect(document.body.textContent).toContain("session-manager-plugin");
  expect(document.body.textContent).not.toContain("SSM session shell");
  expect(button("Save").disabled).toBe(false);
  input("Only show SSM-available instances").click();
  await settle();
  button("Save").click();
  expect(save).toHaveBeenCalledWith({ ...source, only_ssm_available: true });
});

it("requires a region but permits a custom AWS profile", async () => {
  await render({ ...source, profile: "custom-sso", region: "" });
  expect(document.body.textContent).toContain("custom-sso");
  expect(button("Save").disabled).toBe(true);
  const region = input("Region");
  region.value = " eu-west-1 ";
  region.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  button("Save").click();
  expect(save).toHaveBeenCalledWith({ ...source, profile: "custom-sso", region: "eu-west-1" });
});

it("prevents saving when the AWS CLI or session-manager-plugin is unavailable", async () => {
  await render(source, false);
  expect(button("Save").disabled).toBe(true);
  expect(document.body.textContent).toContain("session-manager-plugin");
});

it.each(["docker", "k8s"] as const)("preserves the executable Shell setting for %s", async (platform) => {
  const fields = { id: platform, name: platform, enabled: true, context: "production", shell: "sh" };
  const containerSource: DynamicDiscoverySource = platform === "k8s"
    ? { ...fields, platform, namespace: null }
    : { ...fields, platform };
  await render(containerSource);
  const shell = input("Shell");
  expect(shell.value).toBe("sh");
  shell.value = "bash";
  shell.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
  button("Save").click();
  expect(save).toHaveBeenCalledWith({ ...containerSource, shell: "bash" });
});
