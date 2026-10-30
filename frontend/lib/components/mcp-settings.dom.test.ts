import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import McpSettings from "./McpSettings.svelte";
import * as mcp from "../mcp/store.svelte.ts";
import { setLocale } from "../i18n/index.svelte.ts";

const f = vi.hoisted(() => ({ invoke: vi.fn(), connected: vi.fn(), clipboard: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: f.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async () => () => {}) }));
vi.mock("../stores/app.svelte.ts", () => ({ connectedSessions: f.connected, closeSettings: vi.fn(), sessionIdForTab: vi.fn() }));
vi.mock("../clipboard.ts", () => ({ writeText: f.clipboard }));

let component: ReturnType<typeof mount> | undefined;
async function settle() { flushSync(); await tick(); await Promise.resolve(); flushSync(); }
async function render() {
  const target = document.createElement("div");
  document.body.append(target);
  component = mount(McpSettings, { target });
  await settle();
  return target;
}

beforeEach(() => {
  vi.clearAllMocks();
  setLocale("en");
  f.connected.mockReturnValue([{ sessionId: "ssh", tabId: "tab", type: "ssh", label: "Development" }]);
  f.invoke.mockImplementation(async command => {
    if (command === "mcp_status") return { endpoint: null, sessions: [] };
  });
});
afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  mcp.disconnect();
  document.body.innerHTML = "";
});

describe("MCP settings", () => {
  it("starts the service without separate terminal sharing controls or history", async () => {
    const target = await render();
    expect(target.querySelector('input[type="checkbox"]')).toBeNull();
    expect(target.textContent).not.toContain("Recent commands");
    f.invoke.mockImplementation(async command => {
      if (command === "mcp_start") return { endpoint: { url: "http://127.0.0.1:123/mcp", token: "test-token" }, sessions: [], requests: [] };
    });
    const start = [...target.querySelectorAll("button")].find(b => b.textContent?.includes("Start service"))!;
    await vi.waitFor(() => { flushSync(); expect(start.disabled).toBe(false); });
    expect(f.invoke).not.toHaveBeenCalledWith("mcp_start");
    start.click();
    await vi.waitFor(() => { flushSync(); expect(target.querySelector<HTMLInputElement>('input[type="password"]')?.value).toBe("test-token"); });
  });

  it("copies the endpoint and bearer header without exposing credentials in the page text", async () => {
    f.invoke.mockImplementation(async command => {
      if (command === "mcp_status") return { endpoint: { url: "http://127.0.0.1:123/mcp", token: "test-token" }, sessions: [] };
    });
    const target = await render();
    await vi.waitFor(() => { flushSync(); expect(target.querySelector('input[type="password"]')).not.toBeNull(); });
    const copy = [...target.querySelectorAll("button")].find(b => b.textContent?.includes("Copy connection details"))!;
    await vi.waitFor(() => { flushSync(); expect(copy.disabled).toBe(false); });
    copy.click();
    await vi.waitFor(() => expect(f.clipboard).toHaveBeenCalled());
    expect(JSON.parse(f.clipboard.mock.calls[0][0])).toEqual({ url: "http://127.0.0.1:123/mcp", headers: { Authorization: "Bearer test-token" } });
    expect(target.textContent).not.toContain("test-token");
  });

  it("does not show the previous connection as copied after restarting the service", async () => {
    f.invoke.mockImplementation(async command => {
      if (command === "mcp_status") return { endpoint: { url: "http://127.0.0.1:123/mcp", token: "old-token" }, sessions: [] };
      if (command === "mcp_start") return { endpoint: { url: "http://127.0.0.1:123/mcp", token: "new-token" }, sessions: [] };
    });
    const target = await render();
    const button = (label: string) => [...target.querySelectorAll("button")].find(b => b.textContent?.trim() === label)!;
    await vi.waitFor(() => { flushSync(); expect(button("Copy connection details")?.disabled).toBe(false); });
    button("Copy connection details").click();
    await vi.waitFor(() => { flushSync(); expect(button("Copied")?.disabled).toBe(false); });
    button("Stop service").click();
    await vi.waitFor(() => { flushSync(); expect(button("Start service")?.disabled).toBe(false); });
    button("Start service").click();
    await vi.waitFor(() => { flushSync(); expect(target.querySelector<HTMLInputElement>('input[type="password"]')?.value).toBe("new-token"); });
    expect(button("Copy connection details")).toBeDefined();
    expect(button("Copied")).toBeUndefined();
  });

});
