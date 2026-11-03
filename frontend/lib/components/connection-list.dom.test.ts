import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import ConnectionList from "./ConnectionList.svelte";
import { errMsg, setLocale, t } from "../i18n/index.svelte.ts";
import type { Profile } from "../stores/app.svelte.ts";

const { invokeMock, pickTextFileMock } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  pickTextFileMock: vi.fn(),
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("../pick-file.ts", () => ({ pickTextFile: pickTextFileMock }));

let component: ReturnType<typeof mount> | undefined;
let profiles: Profile[];

const importedProfile: Profile = {
  id: "imported-profile",
  name: "Imported production",
  host: "prod.example.com",
  port: 22,
  credential_id: "credential-1",
  bastion_profile_id: null,
  init_command: null,
  terminal_encoding: "utf-8",
  group_id: null,
  algorithms: { kex: [], key: [], cipher: [], mac: [], compression: [] },
};

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

async function renderList() {
  const target = document.createElement("div");
  document.body.append(target);
  component = mount(ConnectionList, { target });
  await settle();
  await vi.waitFor(() => expect(target.querySelector('[aria-busy="false"]')).not.toBeNull());
  return target;
}

function importButton(target: HTMLElement): HTMLButtonElement {
  const button = Array.from(target.querySelectorAll("button"))
    .find((candidate) => candidate.textContent?.trim() === t("common.import"));
  expect(button, "the connection list exposes the import action").toBeDefined();
  return button!;
}

beforeEach(() => {
  profiles = [];
  invokeMock.mockReset().mockImplementation(async (command: string) => {
    if (command === "list_profiles") return profiles;
    if (command === "import_config") profiles = [importedProfile];
    return [];
  });
  pickTextFileMock.mockReset().mockResolvedValue({ name: "backup.json", text: '{"profiles":[]}' });
  setLocale("en");
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  vi.clearAllTimers();
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("connection list import", () => {
  it("imports the selected JSON and immediately displays the merged connections", async () => {
    const target = await renderList();

    importButton(target).click();

    await vi.waitFor(() => expect(target.textContent).toContain(importedProfile.name));
    expect(pickTextFileMock).toHaveBeenCalledWith({ accept: ".json,application/json" });
    expect(invokeMock).toHaveBeenCalledWith("import_config", { json: '{"profiles":[]}' });
    expect(target.textContent).toContain(t("import_export.imported_from", { path: "backup.json" }));
  });

  it("refreshes partially imported connections and retains multiline error details", async () => {
    const details = "Profile rejected: missing host\nCredential rejected: missing username";
    const error = `__rssh_err__|${JSON.stringify({
      code: "import_partial_failed",
      params: { count: 2, details },
    })}`;
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "list_profiles") return profiles;
      if (command === "import_config") {
        profiles = [importedProfile];
        throw error;
      }
      return [];
    });
    const target = await renderList();
    vi.useFakeTimers();

    importButton(target).click();

    await vi.waitFor(() => expect(target.textContent).toContain(importedProfile.name));
    const message = `${t("toast.error.import")}: ${errMsg(error)}`;
    expect(target.textContent).toContain(message);
    expect(target.textContent).not.toContain("__rssh_err__");

    await vi.advanceTimersByTimeAsync(4000);
    await settle();
    expect(target.textContent).toContain(message);

    await vi.advanceTimersByTimeAsync(11000);
    await settle();
    expect(target.textContent).not.toContain(message);
  });

  it("leaves the configuration alone when the file picker is cancelled", async () => {
    pickTextFileMock.mockResolvedValue(null);
    const target = await renderList();

    importButton(target).click();
    await settle();

    expect(pickTextFileMock).toHaveBeenCalledOnce();
    expect(invokeMock.mock.calls.some(([command]) => command === "import_config")).toBe(false);
    expect(importButton(target).disabled).toBe(false);
    expect(target.textContent).toContain(t("connection.empty"));
  });
});
