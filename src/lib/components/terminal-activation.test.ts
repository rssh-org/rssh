import { readFileSync } from "node:fs";
import { join } from "node:path";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import { describe, expect, it, vi } from "vitest";

const source = readFileSync(join(process.cwd(), "src/lib/components/TerminalPane.svelte"), "utf8");
const keyboardStart = source.lastIndexOf("$effect(() => {", source.indexOf("app.registerSoftKeyboardToggle("));
const keyboardEnd = source.indexOf("\n    });", keyboardStart) + "\n    });".length;
const initialFocusStart = source.indexOf("// Metadata may resolve after activation;");
const initialFocusEnd = source.indexOf("app.registerTerminalControls(", initialFocusStart);
const transpile = (code: string) => ts.transpileModule(code, {
  compilerOptions: { target: ts.ScriptTarget.ES2022 },
}).outputText;
const keyboardEffect = transpile(source.slice(keyboardStart, keyboardEnd));
const initialFocus = transpile(source.slice(initialFocusStart, initialFocusEnd));

describe("terminal activation after asynchronous metadata loading", () => {
  it("registers the delayed keyboard controller without stealing focus", () => {
    const terminal = { focus: vi.fn() };
    const softKeyboard = { toggle: vi.fn() };
    const registerSoftKeyboardToggle = vi.fn();
    const app = {
      activeTabId: () => "ssh:1", settingsActive: () => false,
      registerSoftKeyboardToggle, setSoftKeyboardOpen: vi.fn(),
    };
    const context = {
      $effect: (effect: () => void) => effect(), untrack: (read: () => void) => read(),
      app, tabId: "ssh:1", terminal, softKeyboardRequested: false,
      requestAnimationFrame: vi.fn(), fitTerminal: vi.fn(),
    };
    runInNewContext(keyboardEffect, { ...context, softKeyboard: undefined });
    runInNewContext(keyboardEffect, { ...context, softKeyboard });

    expect(registerSoftKeyboardToggle).toHaveBeenCalledExactlyOnceWith(softKeyboard.toggle);
    expect(terminal.focus).not.toHaveBeenCalled();
  });

  it.each([
    { scenario: "unchanged focus", expected: 1 },
    { scenario: "another input focused", moved: true, expected: 0 },
    { scenario: "another tab activated", inactive: true, expected: 0 },
    { scenario: "settings opened", settings: true, expected: 0 },
    { scenario: "search opened", search: true, expected: 0 },
    { scenario: "snippet picker opened", snippet: true, expected: 0 },
  ])("preserves $scenario when initialization finishes", ({ moved, inactive, settings, search, snippet, expected }) => {
    const focusBeforeInit = {};
    const terminal = { focus: vi.fn() };
    runInNewContext(initialFocus, {
      terminal, focusBeforeInit, tabId: "ssh:1", showSearch: search ?? false,
      document: { activeElement: moved ? {} : focusBeforeInit, body: {} },
      app: {
        activeTabId: () => inactive ? "ssh:2" : "ssh:1",
        settingsActive: () => settings ?? false,
        snippetPickerOpen: () => snippet ?? false,
      },
    });
    expect(terminal.focus).toHaveBeenCalledTimes(expected);
  });
});
