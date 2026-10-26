import { readFileSync } from "node:fs";
import { join } from "node:path";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import { describe, expect, it, vi } from "vitest";

const source = readFileSync(join(process.cwd(), "frontend/lib/components/AppShell.svelte"), "utf8");
const start = source.lastIndexOf("$effect(() => {", source.indexOf("if (!app.capabilities().windowControls)"));
const end = source.indexOf("\n    });", start) + "\n    });".length;
const titleEffectCode = ts.transpileModule(source.slice(start, end), {
  compilerOptions: { target: ts.ScriptTarget.ES2022 },
}).outputText;

type TitleTab = { id: string; label: string };

function titleEffect(options: {
  windowControls?: boolean;
  settings?: boolean;
  workspace?: TitleTab;
  pane?: TitleTab;
  terminalTitle?: string;
  home?: boolean;
} = {}, invoke = vi.fn().mockResolvedValue(undefined)) {
  const home = { id: "home", label: "首页" };
  const tabs = [
    ...(options.home === false ? [] : [home]),
    ...(options.workspace ? [options.workspace] : []),
    ...(options.pane ? [options.pane] : []),
  ];
  const warn = vi.fn();
  const app = {
    capabilities: () => ({ windowControls: options.windowControls ?? true }),
    settingsActive: () => options.settings ?? false,
    tabs: () => tabs,
    workspaceTabs: () => options.workspace ? [options.workspace] : [],
    activeWorkspaceId: () => options.workspace?.id ?? "home",
    activePaneId: () => options.pane?.id ?? options.workspace?.id ?? "home",
    terminalTitle: () => options.terminalTitle ?? "",
    tabLabel: (tab: TitleTab) => tab.label,
  };
  runInNewContext(titleEffectCode, { $effect: (effect: () => void) => effect(), app, invoke, console: { warn } });
  return { invoke, warn };
}

describe("application window title effect", () => {
  it.each([
    { route: "home", options: {}, title: "首页" },
    { route: "settings", options: { settings: true }, title: "Settings" },
    { route: "terminal OSC title", options: { workspace: { id: "ssh:1", label: "SSH Demo" }, terminalTitle: "user@demo" }, title: "user@demo" },
    { route: "terminal workspace", options: { workspace: { id: "ssh:1", label: "SSH Demo" } }, title: "SSH Demo" },
    { route: "pane fallback", options: { home: false, pane: { id: "edit:1", label: "notes.txt" } }, title: "notes.txt" },
    { route: "empty workspace", options: { home: false }, title: "RSSH" },
  ])("sends the $route content title through the host command once", ({ options, title }) => {
    const { invoke } = titleEffect(options);
    expect(invoke).toHaveBeenCalledExactlyOnceWith("set_app_window_title", { title });
  });

  it("does not invoke native window commands when the capability is unavailable", () => {
    const { invoke } = titleEffect({ windowControls: false });
    expect(invoke).not.toHaveBeenCalled();
  });

  it("handles a failed cosmetic title update without an unhandled rejection", async () => {
    const error = new Error("window closed");
    const { warn } = titleEffect({}, vi.fn().mockRejectedValue(error));
    await vi.waitFor(() => expect(warn).toHaveBeenCalledWith("Failed to update window title", error));
  });
});
