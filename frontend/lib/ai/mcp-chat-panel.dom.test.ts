import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import ChatPanel from "./ChatPanel.svelte";
import * as ai from "./store.svelte.ts";
import * as mcp from "../mcp/store.svelte.ts";
import { setLocale } from "../i18n/index.svelte.ts";
import { toast } from "../stores/toast.svelte.ts";
import { registerAlternateScreenProvider, unregisterAlternateScreenProvider } from "../terminal/alternate-screen.ts";

const f = vi.hoisted(() => ({ invoke: vi.fn(), listeners: new Map<string, (event: any) => void>(), open: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: f.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (name, callback) => {
  f.listeners.set(name, callback);
  return () => f.listeners.delete(name);
}) }));
vi.mock("../stores/app.svelte.ts", () => ({
  capabilities: () => ({ mcpServer: true }), sessionIdForTab: () => "transport", openAiPanel: f.open,
  tabs: () => [{ id: "tab", type: "local" }], setActiveWorkspace: vi.fn(), setActivePane: vi.fn(),
}));
vi.mock("../clipboard.ts", () => ({ writeText: vi.fn() }));
const session = { session_id: "transport", tab_id: "tab", nonce: "nonce", name: "Server", kind: "local", shell: { kind: "posix", source: "default" } };
const request = { id: "request", session_id: "transport", tab_id: "tab", nonce: "nonce", kind: "local", title: "Investigate startup failure" };
const info = { tab_id: "tab", instance_id: "actor", target_id: "transport", conversation_id: "conversation", source: "mcp", skill: "general", model: "", provider: "", shell: "posix" };
let component: ReturnType<typeof mount> | undefined;

async function settle() { flushSync(); await tick(); await Promise.resolve(); flushSync(); }
async function render(targetKind: "local" | "ssh" | "serial" | "telnet" = "local") {
  component = mount(ChatPanel, { target: document.body, props: { tabId: "tab", targetKind, targetId: "transport", active: true } });
  await settle();
}

function shellTrigger() {
  return document.querySelector<HTMLButtonElement>('button[aria-label="Conversation shell"]')!;
}

async function chooseShell(label: string) {
  shellTrigger().click();
  await settle();
  const option = [...document.querySelectorAll<HTMLButtonElement>(".shell-select .rssh-select-option")]
    .find(button => button.textContent?.trim() === label)!;
  option.click();
  await settle();
}

beforeEach(async () => {
  vi.clearAllMocks();
  f.listeners.clear();
  setLocale("en");
  ai.activateTab("tab");
  ai.openPanel("tab");
  f.invoke.mockImplementation(async (command, args) => {
    if (command === "ai_settings_get") return { provider: "", model: "", has_api_key: false, danger_mode: false };
    if (command === "mcp_status") return { endpoint: null, sessions: [session], requests: [] };
    if (command === "mcp_accept_session" || command === "ai_session_start" || command === "ai_session_set_shell") return { ...info, shell: args.shell ?? "posix" };
    if (command === "ai_conversations_list") return [];
    if (command === "ai_conversation_timeline") return "[]";
    if (command === "ai_audit_get") return { entries: [] };
    return null;
  });
  await ai.loadSettings();
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  await ai.endConversation("tab");
  unregisterAlternateScreenProvider("tab");
  mcp.disconnect();
  vi.restoreAllMocks();
  document.body.innerHTML = "";
});

describe("conversation shell toolbar", () => {
  it("places the default selector directly before audit and uses the choice for a new AI conversation", async () => {
    await render();
    const select = shellTrigger();
    expect(select.textContent?.trim()).toBe("POSIX");
    expect(select.closest(".shell-select")?.nextElementSibling?.getAttribute("aria-label")).toBe("View audit log");
    await chooseShell("PowerShell");
    expect(select.textContent?.trim()).toBe("PowerShell");
    expect(select.getAttribute("aria-expanded")).toBe("false");
    expect(f.invoke).not.toHaveBeenCalledWith("ai_session_set_shell", expect.anything());
    await ai.startSession({ tabId: "tab", targetKind: "local", targetId: "transport", skill: "", provider: "", model: "", lease: ai.captureSessionLease("tab") });
    expect(f.invoke).toHaveBeenCalledWith("ai_session_start", expect.objectContaining({ shell: "powershell" }));
  });

  it("applies the same draft and live selector to MCP conversations", async () => {
    await mcp.refresh();
    await render();
    const select = shellTrigger();
    await chooseShell("PowerShell");
    f.listeners.get("agent:session_requested:transport")!({ payload: request });
    await settle();
    await mcp.accept("request");
    await settle();
    expect(f.invoke).toHaveBeenCalledWith("mcp_accept_session", { requestId: "request", shell: "powershell" });
    expect(select.textContent?.trim()).toBe("PowerShell");
    await chooseShell("CMD");
    await vi.waitFor(() => {
      flushSync();
      expect(select.textContent?.trim()).toBe("CMD");
    });
    expect(f.invoke).toHaveBeenCalledWith("ai_session_set_shell", { tabId: "tab", instanceId: "actor", shell: "cmd" });
  });

  it("keeps the acknowledged shell while saving and after a failed selection", async () => {
    const errorToast = vi.spyOn(toast, "error").mockImplementation(() => {});
    const originalInvoke = f.invoke.getMockImplementation()!;
    let finishSave!: (value: typeof info) => void;
    const saving = new Promise<typeof info>(resolve => { finishSave = resolve; });
    let failSave = false;
    f.invoke.mockImplementation(async (command, args) => {
      if (command === "ai_session_set_shell") {
        if (failSave) throw new Error("Shell could not be saved");
        return saving;
      }
      return originalInvoke(command, args);
    });
    await ai.startMcpSession({ tabId: "tab", targetKind: "local", targetId: "transport", lease: ai.captureSessionLease("tab") }, "request");
    await render();
    const select = shellTrigger();
    try {
      await chooseShell("PowerShell");
      expect(select.textContent?.trim()).toBe("POSIX");
      expect(select.disabled).toBe(true);
      expect(select.getAttribute("aria-expanded")).toBe("false");
      expect(ai.shellForTab("tab")).toBe("posix");
    } finally {
      finishSave({ ...info, shell: "powershell" });
    }
    await vi.waitFor(() => {
      flushSync();
      expect(select.textContent?.trim()).toBe("PowerShell");
      expect(select.disabled).toBe(false);
    });

    failSave = true;
    await chooseShell("CMD");
    await vi.waitFor(() => {
      flushSync();
      expect(errorToast).toHaveBeenCalledWith("Shell could not be saved");
      expect(select.textContent?.trim()).toBe("PowerShell");
      expect(select.disabled).toBe(false);
      expect(ai.shellForTab("tab")).toBe("powershell");
    });
    select.click();
    await settle();
    expect(select.getAttribute("aria-expanded")).toBe("true");
    expect(document.querySelector('.shell-select .rssh-select-option[aria-pressed="true"]')?.textContent?.trim()).toBe("PowerShell");
  });

  it.each(["serial", "telnet"] as const)("does not offer shell syntax for a %s device", async (kind) => {
    await render(kind);
    expect(document.querySelector('[aria-label="Conversation shell"]')).toBeNull();
  });
});

describe("MCP inside the AI conversation", () => {
  it("does not start control when the panel closes while the previous conversation is saving", async () => {
    const originalInvoke = f.invoke.getMockImplementation()!;
    let finishSave!: () => void;
    const save = new Promise<void>(resolve => { finishSave = resolve; });
    f.invoke.mockImplementation(async (command, args) => {
      if (command === "ai_session_start") return { ...info, source: "ai" };
      if (command === "ai_conversation_save_timeline") return save;
      return originalInvoke(command, args);
    });
    await ai.startSession({
      tabId: "tab", targetKind: "local", targetId: "transport", skill: "general", provider: "", model: "",
      lease: ai.captureSessionLease("tab"),
    });
    await mcp.refresh();
    f.listeners.get("agent:session_requested:transport")!({ payload: request });

    const accepting = mcp.accept("request");
    const rejected = expect(accepting).rejects.toThrow("AI session closed");
    await vi.waitFor(() => expect(f.invoke).toHaveBeenCalledWith("ai_conversation_save_timeline", expect.anything()));
    const closing = ai.closePanel("tab");
    finishSave();
    await Promise.all([rejected, closing]);

    expect(ai.isOpen("tab")).toBe(false);
    expect(ai.sessionForTab("tab")).toBeUndefined();
    expect(f.invoke).not.toHaveBeenCalledWith("mcp_accept_session", expect.anything());
    expect(f.invoke).not.toHaveBeenCalledWith("mcp_session_ready", expect.anything());
  });

  it("shows session consent below the toolbar without a modal and accepts without a provider", async () => {
    await mcp.refresh();
    await render();
    expect(document.querySelector('button[aria-label="Local agent (MCP)"]')).toBeNull();
    expect(document.querySelector('input[type="password"]')).toBeNull();
    expect(f.invoke).not.toHaveBeenCalledWith("mcp_start");
    f.listeners.get("agent:session_requested:transport")!({ payload: request });
    await settle();
    const prompt = document.querySelector('.mcp-request')!;
    expect(prompt.textContent).toContain("Investigate startup failure");
    expect(document.querySelector('[aria-modal="true"]')).toBeNull();
    const accept = [...prompt.querySelectorAll('button')].find(b => b.textContent === "Allow control")!;
    accept.click();
    await vi.waitFor(() => {
      flushSync();
      expect(f.invoke).toHaveBeenCalledWith("mcp_session_ready", { requestId: "request", instanceId: "actor" });
      expect(document.querySelector('textarea')).toBeNull();
    });
    expect(f.invoke).toHaveBeenCalledWith("mcp_accept_session", { requestId: "request", shell: "posix" });
    expect(f.invoke).not.toHaveBeenCalledWith("ai_session_start", expect.anything());
    expect(document.querySelector('button[aria-label="End MCP conversation and revoke control"]')).not.toBeNull();
  });

  it("renders MCP commands with the existing inline command approval card", async () => {
    await ai.startMcpSession({ tabId: "tab", targetKind: "local", targetId: "transport", lease: ai.captureSessionLease("tab") }, "request");
    await render();
    f.listeners.get("ai:command_proposed:tab")!({ payload: {
      id: "command", cmd: "pwd", explain: "Read directory", side_effect: "Read only", kind: "run_command",
      execution: { full_cmd: "pwd", sentinel: "done", timeout_s: 60 }, context_epoch: 0,
    } });
    await settle();
    expect(document.querySelector('.item-command')?.textContent).toContain("pwd");
    expect(document.querySelector('.item-command')?.textContent).toContain("Read directory");
    expect(document.querySelector('[aria-modal="true"]')).toBeNull();
    expect(document.querySelector('textarea')).toBeNull();
  });

  it.each([true, false])("uses the shared danger-mode approval policy for MCP (auto_run_command=%s)", async (autoRunCommand) => {
    const originalInvoke = f.invoke.getMockImplementation()!;
    f.invoke.mockImplementation(async (command, args) => {
      if (command === "ai_settings_get") return {
        provider: "", model: "", has_api_key: false, danger_mode: true, auto_run_command: autoRunCommand,
      };
      return originalInvoke(command, args);
    });
    await ai.loadSettings();
    registerAlternateScreenProvider("tab", () => false);
    await ai.startMcpSession({ tabId: "tab", targetKind: "local", targetId: "transport", lease: ai.captureSessionLease("tab") }, "request");
    await render();
    f.listeners.get("ai:command_proposed:tab")!({ payload: {
      id: "danger-command", cmd: "pwd", explain: "Read directory", side_effect: "Read only", kind: "run_command",
      execution: { full_cmd: "pwd", sentinel: "done", timeout_s: 60 }, context_epoch: 0,
    } });
    await settle();

    if (!autoRunCommand) {
      expect(document.querySelector('.cmd-card .btn-approve')?.textContent).toContain("Run in terminal");
      expect(f.invoke).not.toHaveBeenCalledWith("pty_write", expect.anything());
      return;
    }
    await vi.waitFor(() => expect(f.invoke).toHaveBeenCalledWith("pty_write", {
      sessionId: "transport", data: Array.from(new TextEncoder().encode("pwd\r")),
    }));
    f.listeners.get("pty:data:transport")!({ payload: btoa("pwd\r\n/workspace\r\ndone:0\r\n") });
    await vi.waitFor(() => expect(f.invoke).toHaveBeenCalledWith("ai_command_result", expect.objectContaining({
      tabId: "tab", instanceId: "actor", toolCallId: "danger-command", output: "/workspace", exitCode: 0,
    })));
    expect(f.invoke.mock.calls.filter(([command]) => command === "pty_write")).toHaveLength(1);
  });

  it.each([false, true])("uses the existing download approval and result flow for MCP (auto_download_file=%s)", async (autoDownloadFile) => {
    const originalInvoke = f.invoke.getMockImplementation()!;
    f.invoke.mockImplementation(async (command, args) => {
      if (command === "ai_settings_get") return {
        provider: "", model: "", has_api_key: false, danger_mode: true, auto_download_file: autoDownloadFile,
      };
      return originalInvoke(command, args);
    });
    await ai.loadSettings();
    await ai.startMcpSession({ tabId: "tab", targetKind: "ssh", targetId: "transport", lease: ai.captureSessionLease("tab") }, "request");
    await render("ssh");
    const proposal = { id: "download", remote_path: "/var/log/app.log", max_mb: 1, dest_dir: "/data/diagnose/tab", context_epoch: 0 };
    f.listeners.get("ai:download_proposed:tab")!({ payload: proposal });
    await settle();
    expect(document.body.textContent).toContain(proposal.remote_path);
    expect(document.body.textContent).toContain(proposal.dest_dir);
    expect(document.body.textContent).toContain("max 1 MB");
    expect(document.querySelector('[aria-modal="true"]')).toBeNull();

    const button = (text: string) => [...document.querySelectorAll("button")].find(b => b.textContent?.trim() === text)!;
    if (!autoDownloadFile) {
      expect(button("Allow").disabled).toBe(false);
      expect(f.invoke).not.toHaveBeenCalledWith("ai_command_result", expect.anything());
      button("Reject").click();
      await settle();
      const reason = document.querySelector<HTMLInputElement>('input[placeholder="Reason for rejection (lets AI adjust)"]')!;
      reason.value = "Keep the file on the server";
      reason.dispatchEvent(new Event("input", { bubbles: true }));
      await settle();
      button("Submit").click();
      await vi.waitFor(() => expect(f.invoke).toHaveBeenCalledWith("ai_command_reject", {
        tabId: "tab", instanceId: "actor", toolCallId: "download", reason: reason.value,
      }));
      f.listeners.get("ai:command_rejected:tab")!({ payload: { id: "download", reason: reason.value, context_epoch: 0 } });
      await settle();
      expect(document.body.textContent).toContain("Rejected. Reason: Keep the file on the server");
      expect(f.invoke).not.toHaveBeenCalledWith("ai_command_result", expect.anything());
      return;
    }

    await vi.waitFor(() => expect(f.invoke).toHaveBeenCalledWith("ai_command_result", {
      tabId: "tab", instanceId: "actor", toolCallId: "download", exitCode: 0, output: "", timedOut: false, earlyTerminated: false,
    }));
    const localPath = `${proposal.dest_dir}/app.log`;
    f.listeners.get("ai:download_completed:tab")!({ payload: {
      id: "download", ok: true, local_path: localPath, bytes: 128,
      summary: `Downloaded 128 bytes → ${localPath}`, duration_ms: 5, context_epoch: 0,
    } });
    await settle();
    expect(document.body.textContent).toContain(`Downloaded 128 bytes → ${localPath}`);

    f.listeners.get("ai:download_proposed:tab")!({ payload: { ...proposal, id: "cancelled-download" } });
    await settle();
    await vi.waitFor(() => expect(f.invoke).toHaveBeenCalledWith("ai_command_result", expect.objectContaining({ toolCallId: "cancelled-download" })));
    f.listeners.get("ai:command_rejected:tab")!({ payload: { id: "cancelled-download", reason: "MCP request cancelled", context_epoch: 0 } });
    await settle();
    expect(document.body.textContent).toContain("Rejected. Reason: MCP request cancelled");
    expect(document.body.textContent).toContain(`Downloaded 128 bytes → ${localPath}`);
  });

  it("opens MCP history through the common picker without provider credentials or renewed control", async () => {
    f.invoke.mockImplementation(async command => {
      if (command === "ai_conversations_list") return [{ id: "conversation", title: "Previous diagnosis", source: "mcp", created_at: 1, updated_at: 1 }];
      if (command === "ai_session_start") return info;
      if (command === "ai_conversation_timeline") return "[]";
      if (command === "ai_audit_get") return { entries: [] };
      return null;
    });
    await render();
    await vi.waitFor(() => { flushSync(); expect(document.querySelector('.history-item')).not.toBeNull(); });
    (document.querySelector('.history-item') as HTMLButtonElement).click();
    await vi.waitFor(() => { flushSync(); expect(document.querySelector('textarea')).toBeNull(); });
    expect(f.invoke).toHaveBeenCalledWith("ai_session_start", expect.objectContaining({ provider: "", model: "", resume: "conversation" }));
    expect(f.invoke).not.toHaveBeenCalledWith("mcp_session_ready", expect.anything());
  });
});
