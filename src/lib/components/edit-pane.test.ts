import {readFileSync} from "node:fs";
import {join} from "node:path";
import {describe, expect, it} from "vitest";

const appShellSource = readFileSync(
    join(process.cwd(), "src/lib/components/AppShell.svelte"),
    "utf8",
);
const editPaneSource = readFileSync(
    join(process.cwd(), "src/lib/components/EditPane.svelte"),
    "utf8",
);

// The broadcast editor's document lives only inside its CodeMirror EditorView
// (created empty in onMount, destroyed in onDestroy). If AppShell renders it
// inside the settings/home/forward {:else if} chain, every tab switch unmounts
// the component and the user's text is gone. It must stay mounted per edit tab
// and toggle visibility instead — same contract as terminal workspaces,
// SftpBrowser and ChatPanel instances.
describe("EditPane mounting", () => {
    it("never renders EditPane from the transient route chain", () => {
        expect(appShellSource).not.toContain('activeRouteTab?.type === "edit"');
    });

    it("keeps one EditPane mounted per edit tab via a keyed each", () => {
        expect(appShellSource).toMatch(
            /let editTabs = \$derived\(\s*app\.workspaceTabs\(\)\.filter\(\(tab\) => tab\.type === "edit"\),?\s*\)/,
        );
        const eachStart = appShellSource.indexOf("{#each editTabs as tab (tab.id)}");
        const paneUse = appShellSource.indexOf("<EditPane tabId={tab.id}");
        expect(eachStart).toBeGreaterThan(-1);
        expect(paneUse).toBeGreaterThan(eachStart);
    });

    it("toggles edit panes with class:visible instead of mounting", () => {
        const editBlock = appShellSource.match(
            /\{#each editTabs as tab \(tab\.id\)\}[\s\S]*?\{\/each\}/,
        )?.[0] ?? "";
        expect(editBlock).toContain(
            "class:visible={!app.settingsActive() && tab.id === app.activeWorkspaceId()}",
        );
    });

    it("re-measures CodeMirror when the pane becomes visible again", () => {
        // A display:none container hides CodeMirror's measurements; on reveal
        // the view must be asked to re-measure or the cursor/scrollport drift.
        expect(editPaneSource).toContain("requestMeasure");
    });
});
