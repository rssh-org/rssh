import {readFileSync} from "node:fs";
import {join} from "node:path";
import {describe, expect, it} from "vitest";

const source = readFileSync(
  join(process.cwd(), "src/lib/components/TerminalSplitLayout.svelte"),
  "utf8",
);
const terminalPaneSource = readFileSync(
  join(process.cwd(), "src/lib/components/TerminalPane.svelte"),
  "utf8",
);
const appShellSource = readFileSync(
  join(process.cwd(), "src/lib/components/AppShell.svelte"),
  "utf8",
);

describe("TerminalSplitLayout", () => {
  it("localizes pane controls", () => {
    expect(source).toContain('import {t} from "../i18n/index.svelte.ts"');
    expect(source).toContain('aria-label={t("terminal.pane.activate", {label: tab.label})}');
    expect(source).toContain('title={t("terminal.pane.close")}');
    expect(source).toContain('aria-label={t("terminal.pane.resize")}');
  });

  it("renders the only pane close button inside the child-pane guard", () => {
    const childPaneGuard = source.match(/\{#if tab\.paneOf\}([\s\S]*?)\{\/if\}/)?.[1] ?? "";

    expect(childPaneGuard).toContain('class="close-button"');
    expect(source.match(/class="close-button"/g)).toHaveLength(1);
  });

  it("renders connecting, connected, and disconnected as explicit states", () => {
    expect(source).toContain('{@const connectionStatus = app.terminalConnectionStatus(tab.id)}');
    expect(source).toContain('connectionStatus === "connecting"');
    expect(source).toContain('"common.connecting"');
    expect(source).not.toContain('app.sessionIdForTab(tab.id) ? "common.connected" : "common.disconnected"');
    expect(terminalPaneSource).toContain("app.setTerminalConnectionStatus(tabId, connectionStatus)");
  });

  it("only renders pane headers when multiple panes are visible", () => {
    expect(source).toMatch(
      /\{#if paneLeaves\.length > 1\}\s*\{@const connectionStatus = app\.terminalConnectionStatus\(tab\.id\)\}\s*<header class="pane-header">/,
    );
  });

  it("records the pane actually targeted by a split for failure recovery", () => {
    const splitCurrentPane = appShellSource.match(
      /function splitCurrentPane[\s\S]*?\n    function buildMenu/,
    )?.[0] ?? "";
    const activateWorkspace = splitCurrentPane.indexOf("app.setActiveWorkspace(workspaceId)");
    const captureSource = splitCurrentPane.indexOf("const sourcePaneId = app.activePaneId()");

    expect(activateWorkspace).toBeGreaterThan(-1);
    expect(captureSource).toBeGreaterThan(activateWorkspace);
    expect(splitCurrentPane).toContain("pendingPaneSources[paneId] = sourcePaneId");
  });
});
