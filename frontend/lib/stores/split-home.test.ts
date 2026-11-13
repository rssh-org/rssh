import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { layoutLeaves } from "../terminal/layout.ts";
import type { Tab, TerminalTabType } from "./app.svelte.ts";

const invokeMock = vi.hoisted(() => vi.fn(async () => null as unknown));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

beforeEach(() => {
  vi.resetModules();
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(null);
  const values = new Map<string, string>();
  vi.stubGlobal("localStorage", {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => void values.set(key, value),
    removeItem: (key: string) => void values.delete(key),
  });
  vi.stubGlobal("navigator", { userAgent: "node" });
});

afterEach(() => vi.unstubAllGlobals());

const local = (id: string): Tab => ({ id, type: "local", label: id });
const home = (id: string): Tab => ({ id, type: "home", label: "" });

async function workspaceWithHome() {
  const app = await import("./app.svelte.ts");
  app.addTab(local("root"));
  app.addPane("root", "right", home("home:split"));
  return app;
}

describe("split Home panes", () => {
  it.each([
    ["left", "left", 0, 0.5],
    ["right", "left", 0.5, 0],
    ["top", "top", 0, 0.5],
    ["bottom", "top", 0.5, 0],
  ] as const)("opens Home on the %s without opening a connection", async (side, axis, homePosition, rootPosition) => {
    const app = await import("./app.svelte.ts");
    app.addTab({ id: "root", type: "ssh", label: "Existing SSH", meta: { profileId: "p1" } });
    app.setTerminalConnectionStatus("root", "connected");
    invokeMock.mockClear();

    expect(app.addPane("root", side, home("home:split"))).toBe("home:split");

    const panes = layoutLeaves(app.layoutForWorkspace("root")!);
    expect(panes.find(pane => pane.tabId === "home:split")?.[axis]).toBe(homePosition);
    expect(panes.find(pane => pane.tabId === "root")?.[axis]).toBe(rootPosition);
    expect(app.activeTab()).toMatchObject({ id: "home:split", type: "home", paneOf: "root" });
    expect(app.workspaceTabs().map(tab => tab.id)).toEqual(["root"]);
    expect(app.terminalConnectionStatus("root")).toBe("connected");
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it.each<TerminalTabType>(["ssh", "local", "telnet", "serial", "docker_exec", "kubectl_exec"])(
    "opens a selected %s terminal in the Home pane without adding a workspace",
    async (type) => {
      const app = await workspaceWithHome();
      const selected: Tab = { id: `${type}:selected`, type, label: "Selected connection", meta: { profileId: "selected" } };

      app.openTab(selected, "home:split");

      expect(app.tabs().find(tab => tab.id === "home:split")).toBeUndefined();
      expect(app.activeTab()).toEqual({ ...selected, workspaceId: "root", paneOf: "root" });
      expect(app.activeWorkspaceId()).toBe("root");
      expect(app.paneIdsForWorkspace("root")).toEqual(["root", selected.id]);
      expect(app.workspaceTabs().map(tab => tab.id)).toEqual(["root"]);
      expect(app.tabs().find(tab => tab.id === "home")).toEqual(home("home"));
    },
  );

  it("keeps resized nested pane geometry, siblings and the live root when a Home selection replaces its leaf", async () => {
    const app = await workspaceWithHome();
    app.resizeLayoutPath("root", [], 0.35);
    app.addPane("root", "bottom", local("sibling"));
    app.resizeLayoutPath("root", [1], 0.7);
    app.setTerminalTitle("root", "running job");
    app.setTerminalConnectionStatus("root", "connected");
    const before = layoutLeaves(app.layoutForWorkspace("root")!);

    // A selection is tied to its Home pane even if another pane currently has focus.
    app.setActivePane("sibling");
    app.openTab({ id: "ssh:selected", type: "ssh", label: "Chosen", meta: { profileId: "p2" } }, "home:split");

    expect(layoutLeaves(app.layoutForWorkspace("root")!)).toEqual(
      before.map(pane => pane.tabId === "home:split" ? { ...pane, tabId: "ssh:selected" } : pane),
    );
    expect(app.tabs().find(tab => tab.id === "sibling")).toMatchObject({ type: "local", paneOf: "root" });
    expect(app.terminalTitle("root")).toBe("running job");
    expect(app.terminalConnectionStatus("root")).toBe("connected");
    expect(app.activePaneId()).toBe("ssh:selected");
    expect(app.recentHomeItemIds()).toEqual(["ssh:p2"]);
  });

  it("closes an empty Home pane and its workspace without losing or closing the fixed Home", async () => {
    const app = await workspaceWithHome();
    app.closeTab("home:split");

    expect(app.paneIdsForWorkspace("root")).toEqual(["root"]);
    expect(app.activePaneId()).toBe("root");
    expect(app.tabs().map(tab => tab.id)).toEqual(["home", "root"]);

    app.addPane("root", "left", home("home:another"));
    app.closeTab("root");
    expect(app.tabs()).toEqual([home("home")]);
    expect(app.layoutForWorkspace("root")).toBeNull();
    expect(app.activeWorkspaceId()).toBe("home");
    expect(app.activePaneId()).toBe("home");
    app.closeTab("home");
    expect(app.tabs()).toEqual([home("home")]);
  });

  it("ignores stale and already-consumed Home targets instead of opening extra terminals", async () => {
    const app = await workspaceWithHome();
    app.openTab(local("selected"), "home:split");
    app.openTab(local("duplicate"), "home:split");
    app.openTab(local("wrong-target"), "selected");
    expect(app.tabs().map(tab => tab.id)).toEqual(["home", "root", "selected"]);

    app.closeTab("root");
    app.openTab(local("late-selection"), "home:split");
    expect(app.tabs()).toEqual([home("home")]);
    expect(app.activeTabId()).toBe("home");
  });

  it("opens forwarding in its own workspace and leaves the Home pane available", async () => {
    const app = await workspaceWithHome();
    const layout = app.layoutForWorkspace("root");
    const forward: Tab = { id: "fwd:rule", type: "forward", label: "Forward", meta: { forwardId: "rule" } };

    app.openTab(forward, "home:split");

    expect(app.activeTab()).toEqual(forward);
    expect(app.activeWorkspaceId()).toBe(forward.id);
    expect(app.workspaceTabs().map(tab => tab.id)).toEqual(["root", forward.id]);
    expect(app.layoutForWorkspace("root")).toEqual(layout);
    app.setActiveWorkspace("root");
    expect(app.activePaneId()).toBe("home:split");
  });

  it("keeps selections from the fixed Home as standalone tabs", async () => {
    const app = await workspaceWithHome();
    app.setActiveTab("home");
    app.openTab(local("standalone"));

    expect(app.activeWorkspaceId()).toBe("standalone");
    expect(app.activeTab()).toEqual(local("standalone"));
    expect(app.paneIdsForWorkspace("root")).toEqual(["root", "home:split"]);
  });
});
