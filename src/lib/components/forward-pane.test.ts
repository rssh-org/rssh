import {readFileSync} from "node:fs";
import {join} from "node:path";
import {describe, expect, it} from "vitest";

const appShellSource = readFileSync(
    join(process.cwd(), "src/lib/components/AppShell.svelte"),
    "utf8",
);

// ForwardPane holds the tunnel's runtime entirely in component state
// (activeId, byte counters, rule stats) and its onDestroy stops the forward.
// Rendering it inside the settings/home {:else if} chain tears the tunnel down
// on every tab switch; it must stay mounted per forward tab and toggle
// visibility instead — same contract as edit panes and terminal workspaces.
describe("ForwardPane mounting", () => {
    it("never renders ForwardPane from the transient route chain", () => {
        expect(appShellSource).not.toContain('activeRouteTab?.type === "forward"');
    });

    it("keeps one ForwardPane mounted per forward tab via a keyed each", () => {
        expect(appShellSource).toMatch(
            /let forwardTabs = \$derived\(\s*app\.workspaceTabs\(\)\.filter\(\(tab\) => tab\.type === "forward"\),?\s*\)/,
        );
        const eachStart = appShellSource.indexOf("{#each forwardTabs as tab (tab.id)}");
        const paneUse = appShellSource.indexOf("<ForwardPane tabId={tab.id}");
        expect(eachStart).toBeGreaterThan(-1);
        expect(paneUse).toBeGreaterThan(eachStart);
    });

    it("toggles forward panes with class:visible, still gated on resourcePanesAllowed", () => {
        const forwardBlock = appShellSource.match(
            /\{#each forwardTabs as tab \(tab\.id\)\}[\s\S]*?\{\/each\}/,
        )?.[0] ?? "";
        expect(forwardBlock).toContain(
            "class:visible={!app.settingsActive() && tab.id === app.activeWorkspaceId() && resourcePanesAllowed}",
        );
    });

    it("does not instantiate ForwardPane before startup reconciliation completes", () => {
        // resourcePanesAllowed is false until reconcile_sessions finishes; a
        // ForwardPane mounted in that window races its forward_start against
        // reconcile_sessions({activeIds: []}), which closes every resource the
        // window owns. Mirror the terminal pane gate: keyed shell, inner {#if}.
        const forwardBlock = appShellSource.match(
            /\{#each forwardTabs as tab \(tab\.id\)\}[\s\S]*?\{\/each\}/,
        )?.[0] ?? "";
        const gate = forwardBlock.indexOf("{#if resourcePanesAllowed}");
        const paneUse = forwardBlock.indexOf("<ForwardPane tabId={tab.id}");
        expect(gate).toBeGreaterThan(-1);
        expect(paneUse).toBeGreaterThan(gate);
    });
});
