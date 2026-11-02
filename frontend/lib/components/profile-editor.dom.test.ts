import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import ProfileEditor from "./ProfileEditor.svelte";
import { setLocale } from "../i18n/index.svelte.ts";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("../stores/app.svelte.ts", () => ({
  loadCredentials: async () => [{ id: "credential", name: "Login", username: "root" }],
  loadProfiles: async () => [],
  loadGroups: async () => [],
  navigate: vi.fn(),
}));

const algorithms = {
  kex: ["curve25519-sha256"],
  key: ["ssh-ed25519"],
  cipher: ["aes256-gcm@openssh.com"],
  mac: ["hmac-sha2-256"],
  compression: ["none"],
};
const sourceProfile = {
  id: "source", name: "Server", host: "server.example", port: 22,
  credential_id: "credential", bastion_profile_id: null, init_command: null,
  group_id: null, algorithms,
};
let profile: typeof sourceProfile & { terminal_encoding?: string };
let component: ReturnType<typeof mount> | undefined;

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

async function render(props: { id?: string; copyFromId?: string } = {}) {
  component = mount(ProfileEditor, { target: document.body, props });
  await settle();
  await vi.waitFor(() => expect(document.querySelector(".form")?.getAttribute("aria-busy")).toBe("false"));
}

async function select(id: string, label: string) {
  const trigger = document.getElementById(id) as HTMLButtonElement;
  trigger.click();
  await settle();
  const option = [...trigger.parentElement!.querySelectorAll<HTMLButtonElement>(".rssh-select-option")]
    .find((button) => button.textContent.trim() === label)!;
  option.click();
  await settle();
}

async function save() {
  const button = document.querySelector<HTMLButtonElement>(".form-actions .btn-accent")!;
  expect(button.disabled).toBe(false);
  button.click();
  await settle();
}

beforeEach(() => {
  profile = { ...sourceProfile };
  invokeMock.mockReset().mockImplementation(async (command: string) => {
    if (command === "ssh_algorithm_catalog") return { defaults: algorithms, supported: algorithms };
    if (command === "get_profile") return profile;
    if (command === "create_profile" || command === "update_profile") return null;
    throw new Error(`Unexpected command: ${command}`);
  });
  setLocale("en");
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.innerHTML = "";
});

describe("SSH profile terminal encoding", () => {
  it.each(["utf-8", "gbk"])("creates a profile with %s encoding", async (encoding) => {
    await render();
    expect(document.getElementById("profile-terminal-encoding")?.textContent).toContain("UTF-8");
    for (const [id, value] of [["profile-name", "Server"], ["profile-host", "server.example"]]) {
      const input = document.getElementById(id) as HTMLInputElement;
      input.value = value;
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }
    await select("profile-credential", "Login (root)");
    if (encoding === "gbk") await select("profile-terminal-encoding", "GBK");
    await save();

    expect(invokeMock).toHaveBeenCalledWith("create_profile", {
      profile: expect.objectContaining({ name: "Server", terminal_encoding: encoding }),
    });
  });

  it("defaults an old profile to UTF-8 and saves a changed encoding", async () => {
    await render({ id: "source" });
    expect(document.getElementById("profile-terminal-encoding")?.textContent).toContain("UTF-8");
    await select("profile-terminal-encoding", "GBK");
    await save();

    expect(invokeMock).toHaveBeenCalledWith("update_profile", {
      profile: expect.objectContaining({ id: "source", terminal_encoding: "gbk" }),
    });
  });

  it.each(["edit", "copy"])("preserves GBK when opening a profile to %s", async (mode) => {
    profile.terminal_encoding = "gbk";
    await render(mode === "edit" ? { id: "source" } : { copyFromId: "source" });
    expect(document.getElementById("profile-terminal-encoding")?.textContent).toContain("GBK");
    await save();

    const command = mode === "edit" ? "update_profile" : "create_profile";
    expect(invokeMock).toHaveBeenCalledWith(command, {
      profile: expect.objectContaining({
        id: mode === "edit" ? "source" : expect.not.stringMatching(/^source$/),
        name: mode === "edit" ? "Server" : "Server_copy",
        terminal_encoding: "gbk",
      }),
    });
  });
});
