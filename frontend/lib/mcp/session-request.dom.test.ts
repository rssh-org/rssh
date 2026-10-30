import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import AppShell from "../components/AppShell.svelte";
import * as app from "../stores/app.svelte.ts";
import * as ai from "../ai/store.svelte.ts";
import * as mcp from "./store.svelte.ts";
import { setLocale } from "../i18n/index.svelte.ts";
import { registerAlternateScreenProvider, unregisterAlternateScreenProvider } from "../terminal/alternate-screen.ts";

const f = vi.hoisted(() => ({ invoke: vi.fn(), listeners: new Map<string, (event: any) => void>() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: f.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (name, callback) => {
  f.listeners.set(name, callback);
  return () => f.listeners.delete(name);
}) }));
vi.mock("../components/TerminalSplitLayout.svelte", async () => import("../components/test-fixtures/TerminalWorkspace.svelte"));

let component: ReturnType<typeof mount> | undefined;
const terminal = { session_id: "transport", tab_id: "local:requested-pane", nonce: "nonce", name: "Target", kind: "local", shell: { kind: "posix", source: "launch_config" } };
const actor = { tab_id: terminal.tab_id, instance_id: "actor", target_id: terminal.session_id, conversation_id: "conversation", source: "mcp", skill: "general", provider: "", model: "" };
async function settle() { flushSync(); await tick(); flushSync(); }

beforeEach(() => {
  vi.clearAllMocks();
  f.listeners.clear();
  setLocale("en");
  f.invoke.mockImplementation(async command => {
    if (command === "get_setting") return null;
    if (command === "ai_settings_get") return { provider: "", model: "", danger_mode: false };
    if (command === "mcp_status") return { endpoint: { url: "http://127.0.0.1:1234/mcp", token: "test" }, sessions: [terminal], requests: [] };
    if (command === "mcp_accept_session") return actor;
    return [];
  });
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  mcp.disconnect();
  unregisterAlternateScreenProvider(terminal.tab_id);
  app.closeSettings();
  for (const tab of app.workspaceTabs()) if (tab.id !== "home") app.closeTab(tab.id);
  document.body.innerHTML = "";
});

it.each(["local", "docker_exec", "kubectl_exec"] as const)("authorizes the requested %s split pane and runs commands in that PTY", async (type) => {
  app.addTab({ id: "local:workspace", type: "local", label: "Workspace" });
  app.addPane("local:workspace", "right", { id: terminal.tab_id, type, label: "Target" });
  app.registerSession({ tabId: terminal.tab_id, sessionId: terminal.session_id, type });
  registerAlternateScreenProvider(terminal.tab_id, () => false);
  app.addTab({ id: "local:other", type: "local", label: "Other" });
  app.openSettings();
  component = mount(AppShell, { target: document.body });
  await settle();
  expect(app.settingsActive()).toBe(true);
  expect(app.activeWorkspaceId()).toBe("local:other");
  await mcp.refresh();

  f.listeners.get("agent:session_requested:transport")!({ payload: {
    id: "request", session_id: terminal.session_id, tab_id: terminal.tab_id, nonce: terminal.nonce,
    kind: "local", title: "Investigate the selected terminal",
  } });
  await settle();

  expect(app.settingsActive()).toBe(false);
  expect(app.activeWorkspaceId()).toBe("local:workspace");
  expect(app.activePaneId()).toBe(terminal.tab_id);
  expect(ai.isOpen(terminal.tab_id)).toBe(true);
  await vi.waitFor(() => {
    flushSync();
    const consent = document.querySelector<HTMLElement>(".mcp-request");
    expect(consent?.textContent).toContain("Investigate the selected terminal");
    expect(consent?.closest("[inert]")).toBeNull();
  });
  expect(f.invoke).not.toHaveBeenCalledWith("mcp_accept_session", expect.anything());
  expect(f.invoke).not.toHaveBeenCalledWith("pty_write", expect.anything());
  expect(f.invoke).not.toHaveBeenCalledWith("mcp_start");

  const accept = [...document.querySelectorAll<HTMLButtonElement>(".mcp-request button")]
    .find(button => button.textContent === "Allow control")!;
  accept.click();
  await vi.waitFor(() => expect(f.invoke).toHaveBeenCalledWith("mcp_session_ready", { requestId: "request", instanceId: "actor" }));
  f.listeners.get(`ai:command_proposed:${terminal.tab_id}`)!({ payload: {
    id: "command", kind: "run_command", cmd: "pwd", explain: "Read directory", side_effect: "Read only",
    execution: { full_cmd: "pwd", sentinel: "done", timeout_s: 60 }, context_epoch: 0,
  } });
  await settle();
  document.querySelector<HTMLButtonElement>(".cmd-card .btn-approve")!.click();
  await vi.waitFor(() => expect(f.invoke).toHaveBeenCalledWith("pty_write", {
    sessionId: terminal.session_id, data: Array.from(new TextEncoder().encode("pwd\r")),
  }));
  f.listeners.get(`pty:data:${terminal.session_id}`)!({ payload: btoa("pwd\r\n/container\r\ndone:0\r\n") });
  await vi.waitFor(() => expect(f.invoke).toHaveBeenCalledWith("ai_command_result", expect.objectContaining({
    tabId: terminal.tab_id, instanceId: actor.instance_id, output: "/container", exitCode: 0,
  })));
});
