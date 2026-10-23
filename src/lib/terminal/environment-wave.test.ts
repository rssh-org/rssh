import { beforeEach, describe, expect, it, vi } from "vitest";
import { groupWaveColor, loadTerminalWaveColor, terminalWaveTheme } from "./environment-wave";
import type { Group } from "../stores/app.svelte";
import { PALETTES } from "../themes/palettes";
import { TERM_PRESETS } from "../themes/term-palettes";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

const production: Group = {
  id: "prod", name: "Production", color: "#E05868", sort_order: 0,
  terminal_waves_enabled: true,
};

beforeEach(() => { invokeMock.mockReset(); });

describe("group terminal waves", () => {
  it("preserves each terminal palette's RGB for inverse video and background queries", () => {
    for (const { term } of [...PALETTES, ...TERM_PRESETS]) {
      expect(terminalWaveTheme(term, true)).toEqual({ ...term, background: `${term.background}00` });
      expect(terminalWaveTheme(term, false)).toBe(term);
      expect(term.background).toMatch(/^#[0-9a-f]{6}$/i);
    }
  });

  it.each([
    ["#abc", "#aabbcc00"],
    ["#abcd", "#aabbcc00"],
    ["#FDF6E3", "#FDF6E300"],
    ["#FDF6E380", "#FDF6E300"],
    ["rgb(253, 246, 227)", "rgba(253, 246, 227, 0)"],
    ["rgba(253, 246, 227, 0.5)", "rgba(253, 246, 227, 0)"],
  ])("retains custom background RGB from %s", (background, expected) => {
    const original = { background, foreground: "#31355B", red: "#ff0000" };
    expect(terminalWaveTheme(original, true)).toEqual({ ...original, background: expected });
    expect(original.background).toBe(background);
  });
  it("uses the enabled group's color without a second stored color", () => {
    expect(groupWaveColor(production)).toBe("#E05868");
    expect(groupWaveColor({ ...production, color: "#4A6CF7" })).toBe("#4A6CF7");
  });

  it("omits the decoration when opened with a disabled or missing group", () => {
    expect(groupWaveColor({ ...production, terminal_waves_enabled: false })).toBeNull();
    expect(groupWaveColor(null)).toBeNull();
    expect(groupWaveColor()).toBeNull();
  });

  it("does not insert malformed colors into SVG styles", () => {
    for (const color of ["", "red", "#fff", "#123456; color:red"]) {
      expect(groupWaveColor({ ...production, color })).toBeNull();
    }
  });
});

describe("terminal wave snapshot", () => {
  it.each(["telnet", "serial"] as const)("keeps the cloned %s target's group when its saved profile is reassigned", async (type) => {
    const profile = { id: "profile-1", group_id: "prod", host: "prod.example", port: "/dev/ttyUSB0" };
    const meta = {
      profileId: profile.id, serialProfileId: profile.id,
      groupId: profile.group_id, host: profile.host, port: profile.port,
    };
    profile.group_id = "test";
    profile.host = "test.example";
    profile.port = "/dev/ttyUSB1";
    const clonedMeta = { ...meta };
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "list_telnet_profiles") return [profile];
      if (command === "get_serial_profile") return profile;
      if (command === "list_groups") return [production, { ...production, id: "test", color: "#4A6CF7" }];
      throw new Error(`unexpected command: ${command}`);
    });

    expect(await loadTerminalWaveColor(type, clonedMeta)).toBe(production.color);
    expect(clonedMeta.host).toBe("prod.example");
    expect(clonedMeta.port).toBe("/dev/ttyUSB0");
    expect(invokeMock.mock.calls).toEqual([["list_groups"]]);
  });

  it("uses the current saved SSH profile group, matching ssh_connect", async () => {
    invokeMock.mockImplementation(async (name: string) => {
      if (name === "get_profile") return { group_id: "prod" };
      if (name === "list_groups") return [production];
      throw new Error(`unexpected command: ${name}`);
    });

    expect(await loadTerminalWaveColor("ssh", { profileId: "ssh-1", groupId: "old" })).toBe(production.color);
    expect(invokeMock).toHaveBeenCalledWith("get_profile", { id: "ssh-1" });
  });

  it.each(["telnet", "serial"] as const)("does not reload an ungrouped %s profile", async (type) => {
    for (const groupId of ["", undefined]) {
      const meta = { profileId: "profile-1", serialProfileId: "profile-1", ...(groupId === undefined ? {} : { groupId }) };
      expect(await loadTerminalWaveColor(type, meta)).toBeNull();
    }
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it.each(["local", "docker_exec", "kubectl_exec"] as const)(
    "does not load decoration settings for %s terminals", async (type) => {
      const color = await loadTerminalWaveColor(type, { profileId: "unused" });
      expect(color).toBeNull();
      expect(terminalWaveTheme(PALETTES[0].term, !!color)).toBe(PALETTES[0].term);
      expect(invokeMock).not.toHaveBeenCalled();
    },
  );

  it("does not load groups without a saved profile", async () => {
    expect(await loadTerminalWaveColor("ssh", {})).toBeNull();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it.each([false, true])("keeps absent or disabled groups opaque (enabled=%s)", async (enabled) => {
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "get_profile") return { group_id: enabled ? "missing" : "prod" };
      if (command === "list_groups") return [{ ...production, terminal_waves_enabled: enabled }];
      throw new Error(`unexpected command: ${command}`);
    });

    const color = await loadTerminalWaveColor("ssh", { profileId: "ssh-1" });
    const original = PALETTES[0].term;
    expect(color).toBeNull();
    expect(terminalWaveTheme(original, !!color)).toBe(original);
  });

  it("keeps a loaded color stable while new terminals get the edited group color", async () => {
    const group = { ...production };
    invokeMock.mockImplementation(async (command: string) => {
      if (command === "get_profile") return { group_id: "prod" };
      if (command === "list_groups") return [group];
      throw new Error(`unexpected command: ${command}`);
    });

    const existingColor = await loadTerminalWaveColor("ssh", { profileId: "ssh-1" });
    group.color = "#4A6CF7";
    const newColor = await loadTerminalWaveColor("ssh", { profileId: "ssh-1" });

    expect(existingColor).toBe("#E05868");
    expect(newColor).toBe("#4A6CF7");
    invokeMock.mockClear();
    for (const { term } of PALETTES) terminalWaveTheme(term, !!existingColor);
    expect(existingColor).toBe("#E05868");
    expect(invokeMock).not.toHaveBeenCalled();
  });
});
