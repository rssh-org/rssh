import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import SnippetManager from "./SnippetManager.svelte";
import { setLocale, t } from "../i18n/index.svelte.ts";
import type { Snippet } from "../terminal/snippet-parameters";

const { loadSnippets, updateSnippet } = vi.hoisted(() => ({ loadSnippets: vi.fn(), updateSnippet: vi.fn() }));
vi.mock("../stores/app.svelte.ts", () => ({ loadSnippets, updateSnippet }));
let component: ReturnType<typeof mount>;

function button(label: string) {
  const found = [...document.querySelectorAll<HTMLButtonElement>("button")]
    .find(el => el.textContent.trim() === label || el.getAttribute("aria-label") === label);
  if (!found) throw new Error("Button missing: " + label);
  return found;
}

async function click(label: string) {
  const element = button(label);
  element.focus();
  element.click();
  await tick();
}

async function selectCommandText(field: HTMLTextAreaElement, start: number, end: number) {
  field.focus();
  field.setSelectionRange(start, end);
  field.dispatchEvent(new Event("select"));
  await tick();
}

function highlighted() {
  return Array.from(document.querySelectorAll('.command-highlights[aria-hidden="true"] .parameter-match'),
    match => match.textContent);
}

function input(selector: string, index = 0) {
  return document.querySelectorAll<HTMLTextAreaElement>(selector)[index];
}

function typeInto(field: HTMLInputElement | HTMLTextAreaElement, value: string) {
  field.value = value;
  field.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
}

async function edit(snippet: Snippet) {
  loadSnippets.mockResolvedValue([snippet]);
  component = mount(SnippetManager, { target: document.body });
  await vi.waitFor(() => expect(button("Edit " + snippet.name)).toBeDefined());
  await click("Edit " + snippet.name);
  return document.querySelector("textarea")!;
}

beforeEach(() => {
  setLocale("en");
  loadSnippets.mockReset();
  updateSnippet.mockReset().mockResolvedValue([]);
});
afterEach(async () => {
  if (component) await unmount(component);
  document.body.replaceChildren();
});

it("uses the same add button to register a selection and underline both occurrences", async () => {
  const original = { name: "test", command: "echo yyy xxx zzz xxx" };
  const textarea = await edit(original);
  const first = original.command.indexOf("xxx");
  const buttonCount = document.querySelectorAll("button").length;
  const add = button(t("snippet.editor.add"));
  await selectCommandText(textarea, first, first + 3);
  expect(document.querySelectorAll("button")).toHaveLength(buttonCount);
  expect(button(t("snippet.editor.add"))).toBe(add);
  await click(t("snippet.editor.add"));
  expect(input(".parameter-name").value).toBe("xxx");
  expect(input(".parameter-default").value).toBe("");
  expect(textarea.value).toBe(original.command);
  expect(highlighted()).toEqual(["xxx", "xxx"]);

  await selectCommandText(textarea, original.command.lastIndexOf("xxx"), original.command.length);
  await click(t("snippet.editor.add"));
  expect(document.querySelectorAll(".parameter-row")).toHaveLength(1);
  expect(document.activeElement).toBe(input(".parameter-default"));
  await click(t("common.save"));
  expect(updateSnippet).toHaveBeenCalledExactlyOnceWith({
    ...original, parameters: [{ name: "xxx", defaultValue: null }],
  }, { index: 0, previous: original });
});

it("allows manual literal names and distinct defaults, rejects duplicates and keeps whitespace", async () => {
  const original = { name: "Log", command: "echo a, a, ." };
  await edit(original);
  await click(t("snippet.editor.add"));
  expect(button(t("common.save")).disabled).toBe(true);
  expect(highlighted()).toEqual([]);
  typeInto(input(".parameter-name"), " a, ");
  typeInto(input(".parameter-default"), " optional ");
  await click(t("snippet.editor.add"));
  typeInto(input(".parameter-name", 1), " a, ");
  expect(button(t("common.save")).disabled).toBe(true);
  expect(document.querySelector('[role="alert"]')?.textContent).toBe(t("snippet.editor.duplicate"));
  typeInto(input(".parameter-name", 1), ".");
  typeInto(input(".parameter-default", 1), "old");
  typeInto(input(".parameter-default", 1), "");
  await click(t("common.save"));
  expect(updateSnippet).toHaveBeenCalledExactlyOnceWith({
    ...original, parameters: [{ name: " a, ", defaultValue: " optional " }, { name: ".", defaultValue: null }],
  }, { index: 0, previous: original });
});

it("uses the command selection only while registering from the command field", async () => {
  const textarea = await edit({ name: "Service", command: "status nginx" });
  await selectCommandText(textarea, 7, 12);
  document.querySelector<HTMLInputElement>(".inline-form input")!.focus();
  await click(t("snippet.editor.add"));
  expect(input(".parameter-name").value).toBe("");
  expect(document.activeElement).toBe(input(".parameter-name"));
  expect(highlighted()).toEqual([]);

  textarea.focus();
  await click(t("snippet.editor.add"));
  expect(input(".parameter-name", 1).value).toBe("nginx");
  expect(highlighted()).toEqual(["nginx"]);
});

it("underlines the effective literal matches as the command and overlapping parameters change", async () => {
  const textarea = await edit({
    name: "Service", command: "echo nginx nginx.conf NGINX",
    parameters: ["nginx", "nginx.conf"].map(name => ({ name, defaultValue: null })),
  });
  expect(highlighted()).toEqual(["nginx", "nginx.conf"]);
  typeInto(input(".parameter-name", 1), "{host}");
  expect(highlighted()).toEqual(["nginx", "nginx"]);
  typeInto(textarea, "echo {host} nginx {host}");
  expect(highlighted()).toEqual(["{host}", "nginx", "{host}"]);
  await click("Delete · {host}");
  expect(highlighted()).toEqual(["nginx"]);
  expect(textarea.value).toBe("echo {host} nginx {host}");
});

it("registers a cross-line selection from the original mixed line endings and edits its multi-line default", async () => {
  const match = "🌙first\r\nsecond\rthird";
  const original = { name: "Mixed endings", command: "prefix\r\n" + match + "\nsuffix" };
  const textarea = await edit(original);
  const displayed = textarea.value;
  expect(displayed).toBe("prefix\n🌙first\nsecond\nthird\nsuffix");
  const start = displayed.indexOf("🌙");
  await selectCommandText(textarea, start, displayed.indexOf("\nsuffix"));
  await click(t("snippet.editor.add"));
  expect(input(".parameter-name").value).toBe("🌙first\nsecond\nthird");
  expect(highlighted()).toEqual(["🌙first\nsecond\nthird"]);
  expect(document.querySelector(".command-highlights")?.textContent).toBe(displayed);
  expect(Number(input(".parameter-name").rows)).toBe(1);
  expect(Number(input(".parameter-default").rows)).toBe(1);
  typeInto(input(".parameter-default"), "line 1\nline 2\n");
  typeInto(textarea, displayed + "!");
  await click(t("common.save"));
  expect(updateSnippet).toHaveBeenCalledExactlyOnceWith({
    name: original.name, command: original.command + "!",
    parameters: [{ name: match, defaultValue: "line 1\nline 2\n" }],
  }, { index: 0, previous: original });
});

it("saves manually entered multi-line match text and preserves untouched CRLF in an edited default", async () => {
  const original = {
    name: "Paths", command: "left\nright",
    parameters: [{ name: "left", defaultValue: "first\r\nlast" }],
  };
  await edit(original);
  typeInto(input(".parameter-name"), "left\nright");
  typeInto(input(".parameter-default"), "next\nlast");
  await click(t("common.save"));
  expect(updateSnippet).toHaveBeenCalledExactlyOnceWith({
    ...original, parameters: [{ name: "left\nright", defaultValue: "next\r\nlast" }],
  }, { index: 0, previous: original });
});

it("unregisters the last parameter without rewriting the command or emitting an empty array", async () => {
  const original = { name: "Log", command: "echo {group} {group}", parameters: [{ name: "{group}", defaultValue: "one" }] };
  await edit(original);
  await click("Delete · {group}");
  expect(highlighted()).toEqual([]);
  await click(t("common.save"));
  expect(updateSnippet).toHaveBeenCalledExactlyOnceWith(
    { name: original.name, command: original.command }, { index: 0, previous: original },
  );
});

it("keeps the local draft if the saved snippet changed, and reloads on request", async () => {
  const original = { name: "Paths", command: "pwd" };
  const textarea = await edit(original);
  typeInto(textarea, "pwd -P");
  updateSnippet.mockRejectedValueOnce('__rssh_err__|{"code":"snippet_changed","params":{}}');
  await click(t("common.save"));
  expect(textarea.value).toBe("pwd -P");
  expect(button(t("common.save")).disabled).toBe(true);
  loadSnippets.mockResolvedValue([{ ...original, command: "ls" }]);
  await click(t("snippet.editor.reload"));
  expect(document.querySelector("textarea")).toBeNull();
  expect(document.querySelector(".item-sub")?.textContent).toBe("ls");
});
