import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import AiSettings from "./AiSettings.svelte";
import { setLocale } from "../i18n/index.svelte.ts";
import type { SkillRecord } from "../ai/types.ts";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("../stores/app.svelte.ts", () => ({ capabilities: () => ({ mcpServer: false }) }));

let component: ReturnType<typeof mount> | undefined;
let skills: SkillRecord[];
let systemPrompt: string;
let failSettingsSave: boolean;

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

function button(label: string, root: ParentNode = document) {
  return [...root.querySelectorAll("button")].find((item) => item.textContent?.trim() === label);
}

async function mountSettings() {
  const target = document.createElement("div");
  document.body.append(target);
  component = mount(AiSettings, { target });
  await vi.waitFor(() => {
    flushSync();
    expect(target.querySelectorAll(".skill-item")).toHaveLength(skills.length);
  });
  return target;
}

async function editSkill(id: string) {
  const target = await mountSettings();
  [...target.querySelectorAll<HTMLButtonElement>(".skill-item")]
    .find((item) => item.querySelector(".skill-id")?.textContent === id)!.click();
  await settle();
}

async function setField(id: string, value: string) {
  const field = document.querySelector<HTMLInputElement | HTMLTextAreaElement>(`#${id}`)!;
  expect(field.disabled).toBe(false);
  field.value = value;
  field.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
}

beforeEach(() => {
  setLocale("en");
  skills = [
    { id: "custom", name: "Custom skill", description: "Custom workflow", content: "Original custom content", builtin: false },
  ];
  systemPrompt = "RSSH identity\nGeneral workflow\n";
  failSettingsSave = false;
  invokeMock.mockReset().mockImplementation(async (command: string, args: any) => {
    if (command === "ai_settings_get") return { provider: "", danger_mode: false, system_prompt: systemPrompt };
    if (command === "ai_settings_set") {
      if (failSettingsSave) throw new Error("disk full");
      systemPrompt = args.patch.systemPrompt;
      return;
    }
    if (command === "ai_list_skills") return skills.map((skill) => ({ ...skill }));
    if (command === "ai_save_skill") {
      const existing = skills.find((skill) => skill.id === args.id);
      skills = [...skills.filter((skill) => skill.id !== args.id), { ...existing, ...args, builtin: false }];
      return;
    }
    if (command === "ai_delete_skill") {
      skills = skills.filter((skill) => skill.id !== args.id);
      return;
    }
    return [];
  });
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.innerHTML = "";
});

describe("AI system prompt settings", () => {
  it("loads and saves the system prompt through settings without a general skill", async () => {
    const target = await mountSettings();
    const section = target.querySelector(".system-prompt-settings")!;
    expect(target.querySelector<HTMLTextAreaElement>("#ai-system-prompt")?.value).toBe(systemPrompt);
    expect(target.querySelector(".skill-id")?.textContent).toBe("custom");
    await setField("ai-system-prompt", "Updated identity\n用户方法\n");
    button("Save", section)!.click();
    await vi.waitFor(() => {
      flushSync();
      expect(button("Save", section)?.disabled).toBe(true);
    });
    expect(invokeMock).toHaveBeenCalledWith("ai_settings_set", { patch: { systemPrompt: "Updated identity\n用户方法\n" } });
    expect(systemPrompt).toBe("Updated identity\n用户方法\n");
    expect(invokeMock).not.toHaveBeenCalledWith("ai_save_skill", expect.anything());
  });

  it("cancels edits back to the saved system prompt without writing", async () => {
    const target = await mountSettings();
    await setField("ai-system-prompt", "Unsaved content");
    button("Cancel", target.querySelector(".system-prompt-settings")!)!.click();
    await settle();
    expect(target.querySelector<HTMLTextAreaElement>("#ai-system-prompt")?.value).toBe(systemPrompt);
    expect(invokeMock).not.toHaveBeenCalledWith("ai_settings_set", expect.anything());
  });

  it("keeps the draft after a failed save and allows retry", async () => {
    const target = await mountSettings();
    const section = target.querySelector(".system-prompt-settings")!;
    await setField("ai-system-prompt", "Keep this draft\n");
    failSettingsSave = true;
    button("Save", section)!.click();
    await vi.waitFor(() => {
      flushSync();
      expect(section.textContent).toContain("disk full");
    });
    expect(target.querySelector<HTMLTextAreaElement>("#ai-system-prompt")?.value).toBe("Keep this draft\n");
    expect(systemPrompt).toBe("RSSH identity\nGeneral workflow\n");
    failSettingsSave = false;
    button("Save", section)!.click();
    await vi.waitFor(() => expect(systemPrompt).toBe("Keep this draft\n"));
  });
});

describe("AI user skill settings", () => {
  it("saves edited content through the shared skill command", async () => {
    const id = "custom";
    await editSkill(id);
    expect(document.querySelector<HTMLInputElement>("#sk-id")?.disabled).toBe(true);
    expect(button("Delete")).toBeDefined();
    await setField("sk-name", "Updated workflow");
    await setField("sk-desc", "Updated description");
    await setField("sk-content", "First line\nSecond line\n");
    button("Save", document.querySelector("#sk-content")!.closest(".form")!)!.click();

    await vi.waitFor(() => {
      flushSync();
      expect(document.querySelector("#sk-content")).toBeNull();
    });
    expect(invokeMock).toHaveBeenCalledWith("ai_save_skill", {
      id, name: "Updated workflow", description: "Updated description", content: "First line\nSecond line\n",
    });
    expect(document.querySelectorAll(".skill-item")).toHaveLength(1);
    expect(invokeMock).not.toHaveBeenCalledWith("ai_delete_skill", expect.anything());
  });

  it("discards cancelled skill edits without changing the persisted skill", async () => {
    await editSkill("custom");
    await setField("sk-content", "Unsaved content");
    button("Cancel", document.querySelector("#sk-content")!.closest(".form")!)!.click();
    await settle();
    expect(document.querySelector("#sk-content")).toBeNull();
    expect(skills[0].content).toBe("Original custom content");
    expect(invokeMock).not.toHaveBeenCalledWith("ai_save_skill", expect.anything());
  });

  it("still requires confirmation before deleting a custom skill", async () => {
    await editSkill("custom");
    button("Delete")!.click();
    await settle();
    expect(invokeMock).not.toHaveBeenCalledWith("ai_delete_skill", expect.anything());
    button("Click again to confirm")!.click();
    await vi.waitFor(() => {
      flushSync();
      expect(document.querySelectorAll(".skill-item")).toHaveLength(0);
    });
    expect(invokeMock).toHaveBeenCalledWith("ai_delete_skill", { id: "custom" });
    expect(skills).toHaveLength(0);
  });

  it("creates a user skill", async () => {
    await mountSettings();
    button("+ New", document.querySelector(".skill-header")!)!.click();
    await settle();
    await setField("sk-id", "new-workflow");
    await setField("sk-name", "New workflow");
    await setField("sk-content", "Steps\n");
    button("Save", document.querySelector("#sk-content")!.closest(".form")!)!.click();
    await vi.waitFor(() => {
      flushSync();
      expect(document.querySelectorAll(".skill-item")).toHaveLength(2);
    });
    expect(invokeMock).toHaveBeenCalledWith("ai_save_skill", { id: "new-workflow", name: "New workflow", description: "", content: "Steps\n" });
  });
});
