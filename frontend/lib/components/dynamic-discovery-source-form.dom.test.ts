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
    statuses: { aws_ssm: { platform: "aws_ssm", available, version: "2", error: null } },
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
  expect(document.body.textContent).toContain("Auto (EC2 operating system)");
  expect(button("Save").disabled).toBe(false);
  input("Only show SSM-available instances").click();
  await settle();
  button("Save").click();
  expect(save).toHaveBeenCalledWith({ ...source, only_ssm_available: true, shell: null });
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
  expect(save).toHaveBeenCalledWith({ ...source, profile: "custom-sso", region: "eu-west-1", shell: null });
});

it("prevents saving when the AWS CLI or session-manager-plugin is unavailable", async () => {
  await render(source, false);
  expect(button("Save").disabled).toBe(true);
  expect(document.body.textContent).toContain("session-manager-plugin");
});

it.each([["POSIX (sh / bash / zsh)", "sh"], ["PowerShell", "powershell"], ["cmd.exe", "cmd"]])("saves the actual SSM shell family as %s without changing the AWS session", async (label, shell) => {
  await render();
  expect(document.body.textContent).toContain("does not change the AWS session shell");
  document.querySelector<HTMLButtonElement>('[aria-label="SSM session shell"]')!.click();
  await settle();
  button(label).click();
  await settle();
  button("Save").click();
  expect(save).toHaveBeenCalledWith({ ...source, shell });
});

it("loads an existing shell override and can restore automatic inference", async () => {
  await render({ ...source, shell: "powershell" });
  const select = document.querySelector<HTMLButtonElement>('[aria-label="SSM session shell"]')!;
  expect(select.textContent).toContain("PowerShell");
  select.click();
  await settle();
  button("Auto (EC2 operating system)").click();
  await settle();
  button("Save").click();
  expect(save).toHaveBeenCalledWith({ ...source, shell: null });
});
