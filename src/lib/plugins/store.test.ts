import { beforeEach, describe, expect, it, vi } from "vitest";

// localStorage stub BEFORE the store module loads (it reads positions at
// module top, same as the ai store — node env has no localStorage).
const storage = new Map<string, string>();
(globalThis as Record<string, unknown>).localStorage = {
  getItem: (k: string) => storage.get(k) ?? null,
  setItem: (k: string, v: string) => void storage.set(k, String(v)),
  removeItem: (k: string) => void storage.delete(k),
  clear: () => storage.clear(),
};

const invokeMock = vi.fn();
const convertFileSrcMock = vi.fn((p: string) => `asset://localhost${p}`);
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
  convertFileSrc: (p: string) => convertFileSrcMock(p),
}));

// Dynamic import: the static form hoists above the localStorage stub above
// (the store reads positions at module top, same trap as the ai store).
const store = await import("./store.svelte.ts");
// Type-only import is erased at runtime — safe to reference the shapes.
import type { PluginInfo } from "./store.svelte.ts";

function plugin(id: string, area: "side" | "strip", enabled = true, sort_order = 0): PluginInfo {
  return {
    id, name: id, version: "1.0.0", description: "", author: "",
    area, preview: "preview.html", enabled, installed_at: 0, sort_order,
  };
}

beforeEach(() => {
  storage.clear();
  invokeMock.mockReset();
  convertFileSrcMock.mockClear();
  convertFileSrcMock.mockImplementation((p: string) => `asset://localhost${p}`);
});

describe("registry", () => {
  it("load() populates plugins and the entry-url root", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "list_plugins") return Promise.resolve([plugin("mon", "side")]);
      if (cmd === "plugins_root") return Promise.resolve("/home/u/.rssh/plugins");
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    await store.load();
    expect(store.plugins()).toHaveLength(1);
    expect(store.entryUrl(store.plugins()[0])).toBe(
      "asset://localhost/home/u/.rssh/plugins/mon/index.html",
    );
  });

  it("install() round-trips base64 + target area through install_plugin", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "install_plugin") return Promise.resolve(plugin("new", "strip"));
      if (cmd === "list_plugins") return Promise.resolve([plugin("new", "strip")]);
      if (cmd === "plugins_root") return Promise.resolve("/p");
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    await store.install("aGVsbG8=", "strip");
    expect(invokeMock).toHaveBeenCalledWith("install_plugin", {base64Zip: "aGVsbG8=", area: "strip"});
    expect(store.plugins()[0].id).toBe("new");
  });

  it("setEnabled reverts optimistically when the backend rejects", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "list_plugins") return Promise.resolve([plugin("mon", "side")]);
      if (cmd === "plugins_root") return Promise.resolve("/p");
      if (cmd === "set_plugin_enabled") return Promise.reject(new Error("boom"));
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    await store.load();
    await expect(store.setEnabled("mon", false)).rejects.toThrow("boom");
    expect(store.plugins()[0].enabled).toBe(true);
  });

  it("uninstall() removes the plugin from the registry", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "list_plugins") return Promise.resolve([plugin("mon", "side")]);
      if (cmd === "plugins_root") return Promise.resolve("/p");
      if (cmd === "uninstall_plugin") return Promise.resolve(undefined);
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    await store.load();
    await store.uninstall("mon");
    expect(store.plugins()).toHaveLength(0);
  });

  it("area filters only return enabled plugins", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "list_plugins")
        return Promise.resolve([plugin("a", "side"), plugin("b", "side", false), plugin("c", "strip")]);
      if (cmd === "plugins_root") return Promise.resolve("/p");
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    await store.load();
    expect(store.sidePlugins().map(p => p.id)).toEqual(["a"]);
    expect(store.stripPlugins().map(p => p.id)).toEqual(["c"]);
  });
});

describe("per-tab open state", () => {
  it("side and strip open independently and dispose per tab", () => {
    store.openSide("ssh:1");
    expect(store.isSideOpen("ssh:1")).toBe(true);
    expect(store.isStripOpen("ssh:1")).toBe(false);

    store.openStrip("ssh:1");
    expect(store.isStripOpen("ssh:1")).toBe(true);

    store.closeSide("ssh:1");
    expect(store.isSideOpen("ssh:1")).toBe(false);
    expect(store.isStripOpen("ssh:1")).toBe(true);

    store.disposeTab("ssh:1");
    expect(store.isStripOpen("ssh:1")).toBe(false);
  });

  it("disposeTab also drops the side width preference", () => {
    store.setSideWidth("ssh:1", 400);
    store.disposeTab("ssh:1");
    expect(store.sideWidth("ssh:1")).toBe(null);
  });
});

describe("auto-open preferences", () => {
  it("openForNewTab follows the per-area toggles and persists them", () => {
    store.setSideAutoOpen(true);
    store.setStripAutoOpen(false);
    expect(storage.get("plugin_side_auto_open")).toBe("true");
    expect(storage.get("plugin_strip_auto_open")).toBe("false");

    store.openForNewTab("ssh:1");
    expect(store.isSideOpen("ssh:1")).toBe(true);
    expect(store.isStripOpen("ssh:1")).toBe(false);
  });

  it("closeAllPanels wipes both areas but keeps width preferences", () => {
    store.setSideAutoOpen(true);
    store.setStripAutoOpen(true);
    store.openForNewTab("ssh:1");
    store.openForNewTab("ssh:2");
    store.setSideWidth("ssh:1", 400);

    store.closeAllPanels();
    expect(store.isSideOpen("ssh:1")).toBe(false);
    expect(store.isSideOpen("ssh:2")).toBe(false);
    expect(store.isStripOpen("ssh:1")).toBe(false);
    expect(store.sideWidth("ssh:1")).toBe(400);
  });
});

describe("positions & sizes", () => {
  it("defaults and persistence round-trip through localStorage", () => {
    expect(store.sidePosition()).toBe("right");
    expect(store.stripPosition()).toBe("bottom");

    store.setSidePosition("left");
    store.setStripPosition("top");

    // A fresh module load re-reads storage; positions are not session state.
    expect(storage.get("plugin_side_position")).toBe("left");
    expect(storage.get("plugin_strip_position")).toBe("top");
  });

  it("previewUrl resolves the package-relative preview document", () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "list_plugins") return Promise.resolve([plugin("mon", "side")]);
      if (cmd === "plugins_root") return Promise.resolve("/root/plugins");
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    return store.load().then(() => {
      const p = store.plugins()[0];
      expect(store.previewUrl(p)).toBe("asset://localhost/root/plugins/mon/preview.html");
      expect(store.entryUrl(p)).toBe("asset://localhost/root/plugins/mon/index.html");
      expect(store.previewUrl({...p, preview: ""})).toBeNull();
    });
  });
});

describe("movePluginTo", () => {
  // Installed inside each it(): the outer beforeEach mockReset() wipes any
  // implementation installed at describe scope.
  let orderArg: string[] | null = null;
  function mockOrderBackend(): void {
    invokeMock.mockImplementation((cmd: string, args: unknown) => {
      if (cmd === "set_plugin_order") {
        orderArg = (args as {ids: string[]}).ids;
        return Promise.resolve(undefined);
      }
      if (cmd === "list_plugins") {
        // Post-move listing comes back reordered (backend ORDER BY sort_order).
        const ids = orderArg ?? ["a", "b", "c"];
        return Promise.resolve(
          ids.map((id, i) => plugin(id, id === "b" ? "strip" : "side", true, i)),
        );
      }
      if (cmd === "plugins_root") return Promise.resolve("/p");
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
  }

  it("drops onto a target within the area and persists the full order", async () => {
    mockOrderBackend();
    await store.load();
    // a dragged onto c: a takes c's slot, c is pushed up. b (strip) untouched.
    await store.movePluginTo("a", "c");
    expect(orderArg).toEqual(["c", "a"]);
    expect(store.sidePlugins().map(p => p.id)).toEqual(["c", "a"]);
  });

  it("ignores drops across areas, onto self and onto unknown ids", async () => {
    mockOrderBackend();
    orderArg = null; // start from the default listing [a, b, c]
    await store.load();
    await store.movePluginTo("a", "b"); // side onto strip
    await store.movePluginTo("a", "a"); // onto itself
    await store.movePluginTo("a", "nope"); // unknown target
    expect(orderArg).toBeNull();
    expect(store.sidePlugins().map(p => p.id)).toEqual(["a", "c"]);
  });
});

describe("hostSupported", () => {
  it("true when convertFileSrc maps to a protocol URL", () => {
    expect(store.hostSupported()).toBe(true);
  });

  it("false under the JCEF/browser shim (identity convertFileSrc)", () => {
    convertFileSrcMock.mockImplementation((p: string) => p);
    expect(store.hostSupported()).toBe(false);
  });
});
