import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { flushSync, mount, unmount } from "svelte";
import IncidentReports from "./IncidentReports.svelte";
import { setLocale } from "../i18n/index.svelte.ts";
import { requestBack } from "../stores/back.svelte.ts";
import type { IncidentReport } from "../stores/incident-reports.svelte.ts";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

let host: HTMLDivElement;
let page: ReturnType<typeof mount> | undefined;
let db: (IncidentReport & { isCurrent: boolean })[];

function mountPage() {
  page = mount(IncidentReports, { target: host });
}

async function leavePage() {
  await unmount(page!);
  page = undefined;
}

function button(text: string) {
  return [...host.querySelectorAll("button")].find(el => el.textContent.trim() === text);
}

function rowAction(action: "Edit" | "Delete", label: string) {
  return [...host.querySelectorAll("button")].find(el => el.getAttribute("aria-label") === `${action} ${label}`);
}

async function listItem(title: string) {
  await vi.waitFor(() => {
    flushSync();
    expect(rowAction("Edit", title)?.disabled).toBe(false);
  });
  return rowAction("Edit", title)!.closest("li")!;
}

async function open(title: string) {
  await listItem(title);
  rowAction("Edit", title)!.click();
  await vi.waitFor(() => {
    flushSync();
    expect(host.querySelector(".inline-form .item-name")?.textContent).toBe(title);
  });
  return host.querySelector("textarea")!;
}

function edit(field: HTMLTextAreaElement, value: string) {
  field.value = value;
  field.dispatchEvent(new Event("input", { bubbles: true }));
  flushSync();
}

beforeEach(() => {
  setLocale("en");
  host = document.createElement("div");
  document.body.append(host);
  db = [
    { id: "first", title: "First report", createdAt: 1700000000000, isCurrent: true, content: "First report\n\nrequest\n  original body" },
    { id: "second", title: "Second report", createdAt: 1700000000001, isCurrent: false, content: "Second report\n\nresponse" },
  ];
  const summaries = () => db.map(({ content, ...summary }) => ({ ...summary }));
  invoke.mockReset().mockImplementation(async (command, args) => {
    switch (command) {
      case "list_incident_reports": return summaries();
      case "load_incident_report": return structuredClone(db.find(item => item.id === args.id));
      case "select_incident_report":
        db.forEach(item => { item.isCurrent = item.id === args.id; });
        return summaries();
      case "delete_incident_report":
        db = db.filter(item => item.id !== args.id);
        return summaries();
      case "update_incident_report": {
        const report = db.find(item => item.id === args.id)!;
        if (!report.content.startsWith(args.previousContent)) {
          throw new Error('__rssh_err__|{"code":"incident_report_edit_conflict"}');
        }
        report.content = args.content + report.content.slice(args.previousContent.length);
        report.title = report.content.split("\n", 1)[0].trim();
        return summaries();
      }
      default: throw new Error(`Unexpected command ${command}`);
    }
  });
});

afterEach(async () => {
  if (page) await leavePage();
  host.remove();
});

it("opens only from the edit icon and provides a title-specific accessible name for each row action", async () => {
  mountPage();
  const row = await listItem("First report");
  expect(row.classList.contains("card")).toBe(true);
  const editButton = rowAction("Edit", "First report")!;
  const deleteButton = rowAction("Delete", "First report")!;
  expect(editButton.title).toBe("Edit");
  expect(deleteButton.title).toBe("Delete");
  expect(editButton.querySelector("svg")).not.toBeNull();
  expect(deleteButton.classList.contains("btn-danger")).toBe(true);
  row.click();
  row.querySelector<HTMLElement>(".item-name")!.click();
  flushSync();
  expect(host.querySelector("textarea")).toBeNull();
  expect(invoke.mock.calls.map(call => call[0])).toEqual(["list_incident_reports"]);

  const field = await open("First report");
  expect(field.value).toBe(db[0].content);
  expect(field.closest("li")!.classList.contains("card")).toBe(true);
  expect(host.querySelectorAll(".inline-form")).toHaveLength(1);
  expect(rowAction("Delete", "Second report")?.disabled).toBe(true);
  expect(rowAction("Edit", "Second report")?.disabled).toBe(false);
  field.setSelectionRange(0, 5);
  expect(field.value.slice(field.selectionStart, field.selectionEnd)).toBe("First");
  host.querySelector<HTMLElement>(".inline-form .item-name")!.click();
  flushSync();
  expect(host.querySelector("textarea")).toBe(field);
  expect(button("Save")).toBeDefined();
  expect(button("Cancel")).toBeDefined();
});

it("saves the Markdown on explicit Save and reflects its first line in the row", async () => {
  mountPage();
  const field = await open("Second report");
  edit(field, "\n\nIncomplete title");
  expect(button("Save")!.disabled).toBe(true);
  expect(host.querySelector(".inline-form .item-name")!.textContent).toBe("Second report");
  const draft = "# Gateway timeout\n\nAssessment\n\n```\n  response\n```\n";
  edit(field, draft);
  expect(host.querySelector(".inline-form .item-name")!.textContent).toBe("# Gateway timeout");
  expect(db[1].title).toBe("Second report");
  db[1].content += "\n\nConcurrent excerpt";
  button("Save")!.click();
  await listItem("# Gateway timeout");
  expect(host.querySelector("textarea")).toBeNull();
  expect(db[1].content).toBe(draft + "\n\nConcurrent excerpt");
  expect((await open("# Gateway timeout")).value).toBe(db[1].content);
});

it("discards edits on Cancel, switching editors, system Back, and leaving the page", async () => {
  mountPage();
  const original = db[0].content;
  edit(await open("First report"), "Canceled draft");
  button("Cancel")!.click();
  edit(await open("First report"), "Switched draft");
  expect((await open("Second report")).value).toBe(db[1].content);
  const first = await open("First report");
  expect(first.value).toBe(original);
  edit(first, "Back draft");
  expect(requestBack()).toBe(true);
  flushSync();
  expect(requestBack()).toBe(false);
  const beforeExit = await open("First report");
  expect(beforeExit.value).toBe(original);
  edit(beforeExit, "Unmounted draft");
  await leavePage();
  mountPage();
  expect((await open("First report")).value).toBe(original);
  expect(invoke.mock.calls.some(call => call[0] === "update_incident_report")).toBe(false);
});

it("keeps a conflicting edit open with its unsaved text and Cancel available", async () => {
  mountPage();
  const field = await open("First report");
  edit(field, "First report\n\nMy draft");
  db[0].title = "Renamed elsewhere";
  db[0].content = "Renamed elsewhere\n\nChanged by another window";
  button("Save")!.click();
  await vi.waitFor(() => expect(host.querySelector('[role="alert"]')?.textContent).toContain("another window"));
  expect(field.value).toBe("First report\n\nMy draft");
  expect(rowAction("Edit", "Second report")!.disabled).toBe(true);
  expect(rowAction("Delete", "Second report")!.disabled).toBe(true);
  button("Cancel")!.click();
  expect((await open("Renamed elsewhere")).value).toBe(db[0].content);
});

it("refreshes a remote rename and cancels an externally deleted editor when the window regains focus", async () => {
  mountPage();
  const field = await open("First report");
  edit(field, "Draft in this window");
  db[1].title = "Renamed";
  window.dispatchEvent(new Event("focus"));
  await listItem("Renamed");
  expect(field.value).toBe("Draft in this window");
  db = db.filter(item => item.id !== "first");
  window.dispatchEvent(new Event("focus"));
  await vi.waitFor(() => {
    flushSync();
    expect(host.querySelector("textarea")).toBeNull();
    expect(rowAction("Delete", "Renamed")!.disabled).toBe(false);
  });
  expect(requestBack()).toBe(false);
  expect(invoke.mock.calls.some(call => call[0] === "update_incident_report")).toBe(false);
});

it("deletes the selected report immediately, including a report with a duplicate title", async () => {
  db.forEach(item => { item.title = "Duplicate"; });
  mountPage();
  await listItem("Duplicate · first");
  rowAction("Delete", "Duplicate · second")!.focus();
  rowAction("Delete", "Duplicate · second")!.click();
  await listItem("Duplicate");
  await vi.waitFor(() => expect(document.activeElement).toBe(rowAction("Edit", "Duplicate")));
  expect(invoke).toHaveBeenCalledWith("delete_incident_report", { id: "second" });
  expect(db.map(item => item.id)).toEqual(["first"]);
  rowAction("Delete", "Duplicate")!.focus();
  rowAction("Delete", "Duplicate")!.click();
  await vi.waitFor(() => {
    flushSync();
    expect(host.textContent).toContain("terminal context menu");
    expect(document.activeElement).toBe(host.querySelector(".status"));
  });
  expect(host.querySelector("textarea")).toBeNull();
});

it("preserves the original line endings of untouched log evidence after editing the report title", async () => {
  const body = "\n\n## gateway\n\n```\nPOST /records\r\nX-Request: 7\r\n\r\nmessage\rnext\n```";
  db[0].content = "First report" + body;
  mountPage();
  const input = await open("First report");
  edit(input, "Renamed" + body.replace(/\r\n?/g, "\n"));
  button("Save")!.click();
  await listItem("Renamed");
  expect(db[0].content).toBe("Renamed" + body);
});

it("replaces an initial list loading error with the refreshed list on window focus", async () => {
  invoke.mockRejectedValueOnce(new Error("Temporary database error"));
  mountPage();
  await vi.waitFor(() => expect(host.querySelector('[role="alert"]')?.textContent).toContain("database"));
  window.dispatchEvent(new Event("focus"));
  await listItem("First report");
  expect(host.querySelector('[role="alert"]')).toBeNull();
});

it("removes a report deleted in another window when opening the stale row fails", async () => {
  mountPage();
  await listItem("First report");
  db.shift();
  invoke.mockRejectedValueOnce(new Error('__rssh_err__|{"code":"incident_report_not_found"}'));
  rowAction("Edit", "First report")!.click();
  await vi.waitFor(() => {
    flushSync();
    expect(rowAction("Edit", "First report")).toBeUndefined();
    expect(rowAction("Delete", "Second report")?.disabled).toBe(false);
  });
});

it("keeps the keyboard at the edited row when entering the editor and after Cancel or Save", async () => {
  mountPage();
  await listItem("Second report");
  rowAction("Edit", "Second report")!.focus();
  const field = await open("Second report");
  expect(document.activeElement).toBe(field);

  button("Cancel")!.focus();
  button("Cancel")!.click();
  await listItem("Second report");
  await vi.waitFor(() => expect(document.activeElement).toBe(rowAction("Edit", "Second report")));

  edit(await open("Second report"), "Updated report");
  button("Save")!.focus();
  button("Save")!.click();
  await listItem("Updated report");
  await vi.waitFor(() => expect(document.activeElement).toBe(rowAction("Edit", "Updated report")));
});
