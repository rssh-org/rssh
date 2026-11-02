import { beforeEach, describe, expect, it, vi } from "vitest";

const f = vi.hoisted(() => ({
  invoke: vi.fn(), sessionId: vi.fn(), open: vi.fn(), end: vi.fn(), start: vi.fn(),
  lease: vi.fn(), current: vi.fn(), all: vi.fn(), listeners: new Map<string, (e: any) => void>(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: f.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn(async (name, callback) => {
  f.listeners.set(name, callback);
  return () => f.listeners.delete(name);
}) }));
vi.mock("../stores/app.svelte.ts", () => ({
  sessionIdForTab: f.sessionId, openAiPanel: f.open,
  tabs: () => [{ id: "tab", type: "ssh" }], setActiveWorkspace: vi.fn(), setActivePane: vi.fn(),
}));
vi.mock("../ai/store.svelte.ts", () => ({
  endConversation: f.end, startMcpSession: f.start, captureSessionLease: f.lease,
  sessionForTab: f.current, listAllSessions: f.all,
}));
const session = { session_id: "transport", tab_id: "tab", nonce: "nonce", name: "Server", kind: "ssh", shell: { kind: "posix", source: "fallback" } };
const request = { id: "request", session_id: "transport", tab_id: "tab", nonce: "nonce", kind: "ssh", title: "Debug server" };
const info = { tab_id: "tab", instance_id: "actor", target_id: "transport", conversation_id: "conversation", source: "mcp" };

beforeEach(() => {
  vi.resetModules();
  vi.clearAllMocks();
  f.listeners.clear();
  f.sessionId.mockReturnValue("transport");
  f.end.mockResolvedValue(undefined);
  f.start.mockResolvedValue(info);
  f.lease.mockReturnValue({});
  f.current.mockReturnValue(info);
  f.all.mockReturnValue([]);
  f.invoke.mockImplementation(async command => {
    if (command === "mcp_status") return { endpoint: { url: "http://127.0.0.1:1234/mcp", token: "test" }, sessions: [session], requests: [] };
    if (command === "mcp_register_session") return session;
  });
});

async function ready() {
  const store = await import("./store.svelte.ts");
  await store.refresh();
  f.listeners.get("agent:session_requested:transport")!({ payload: request });
  return store;
}

describe("MCP conversation authorization", () => {
  it("opens the existing AI panel but starts no actor until accepted", async () => {
    const store = await ready();
    expect(store.requestsForTab("tab")).toEqual([expect.objectContaining(request)]);
    expect(f.open).toHaveBeenCalledWith("tab");
    expect(f.start).not.toHaveBeenCalled();
  });

  it("waits for the AI store to attach its listeners before releasing open_session", async () => {
    const store = await ready();
    let finish!: (value: typeof info) => void;
    f.start.mockImplementation(() => new Promise(resolve => { finish = resolve; }));
    const accept = store.accept("request");
    await vi.waitFor(() => expect(f.start).toHaveBeenCalled());
    expect(f.end).toHaveBeenCalledWith("tab");
    expect(f.invoke).not.toHaveBeenCalledWith("mcp_session_ready", expect.anything());
    finish(info);
    await accept;
    expect(f.invoke).toHaveBeenCalledWith("mcp_session_ready", { requestId: "request", instanceId: "actor" });
    expect(store.requestsForTab("tab")).toEqual([]);
  });

  it("does not authorize an old target after reconnect", async () => {
    const store = await ready();
    f.sessionId.mockReturnValue("replacement");
    await expect(store.accept("request")).rejects.toThrow();
    expect(f.start).not.toHaveBeenCalled();
  });

  it("rejects only the open request and leaves the current AI conversation alone", async () => {
    const store = await ready();
    await store.reject("request");
    expect(f.invoke).toHaveBeenCalledWith("mcp_reject_session", { requestId: "request" });
    expect(f.end).not.toHaveBeenCalled();
    expect(store.requestsForTab("tab")).toEqual([]);
  });

  it("drops cancelled, duplicate and foreign target requests", async () => {
    const store = await ready();
    f.listeners.get("agent:session_requested:transport")!({ payload: request });
    f.listeners.get("agent:session_requested:transport")!({ payload: { ...request, id: "foreign", nonce: "other" } });
    expect(store.requestsForTab("tab")).toHaveLength(1);
    f.listeners.get("agent:session_cancelled:transport")!({ payload: { id: "request" } });
    expect(store.requestsForTab("tab")).toEqual([]);
  });

  it("registers connected terminals without a user sharing checkbox or per-command path", async () => {
    const store = await import("./store.svelte.ts");
    await store.reconcileSessions([{ sessionId: "transport", tabId: "tab", type: "ssh", label: "Server" }]);
    expect(f.listeners.has("agent:session_requested:transport")).toBe(true);
    expect(f.invoke).toHaveBeenCalledWith("mcp_register_session", { sessionId: "transport", tabId: "tab", label: "Server" });
    expect(f.invoke).not.toHaveBeenCalledWith("mcp_share_session", expect.anything());
  });
});


it("closes only the newly adopted actor if ready fails", async () => {
  const store = await ready();
  f.invoke.mockImplementation(async command => {
    if (command === "mcp_session_ready") throw new Error("request cancelled");
  });
  await expect(store.accept("request")).rejects.toThrow("request cancelled");
  expect(f.end.mock.calls).toEqual([["tab"], ["tab"]]);
});

it("does not close a replacement actor after a late ready failure", async () => {
  const store = await ready();
  f.invoke.mockImplementation(async command => {
    if (command === "mcp_session_ready") {
      f.current.mockReturnValue({ ...info, instance_id: "replacement" });
      throw new Error("request cancelled");
    }
  });
  await expect(store.accept("request")).rejects.toThrow("request cancelled");
  expect(f.end.mock.calls).toEqual([["tab"]]);
});

it("closes the adopted actor when the request is cancelled while listeners attach", async () => {
  const store = await ready();
  f.start.mockImplementation(async () => {
    f.listeners.get("agent:session_cancelled:transport")!({ payload: { id: "request" } });
    return info;
  });
  await expect(store.accept("request")).rejects.toThrow();
  expect(f.end.mock.calls).toEqual([["tab"], ["tab"]]);
  expect(f.invoke).not.toHaveBeenCalledWith("mcp_session_ready", expect.anything());
});

it("keeps a rapid restart behind the final timeline save and endpoint stop", async () => {
  const store = await ready();
  let finish!: () => void;
  f.all.mockReturnValue([info]);
  f.end.mockImplementation(() => new Promise<void>(resolve => { finish = resolve; }));
  f.invoke.mockImplementation(async command => {
    if (command === "mcp_start") return { endpoint: { url: "http://127.0.0.1:4321/mcp", token: "new" }, sessions: [session], requests: [] };
  });
  const stopping = store.stop();
  const starting = store.start();
  await vi.waitFor(() => expect(f.end).toHaveBeenCalled());
  expect(f.invoke).not.toHaveBeenCalledWith("mcp_start");
  finish();
  await Promise.all([stopping, starting]);
  const sequence = f.invoke.mock.calls.map(([command]) => command);
  expect(sequence.indexOf("mcp_stop")).toBeLessThan(sequence.indexOf("mcp_start"));
  expect(store.endpoint()?.token).toBe("new");
});
