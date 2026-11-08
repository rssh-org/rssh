import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { flushSync, mount, tick, unmount } from "svelte";
import AiSettings from "./AiSettings.svelte";
import { setLocale } from "../i18n/index.svelte.ts";
import type { AiProviderRecord } from "../ai/types.ts";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("../stores/app.svelte.ts", () => ({ capabilities: () => ({ mcpServer: false }) }));

let component: ReturnType<typeof mount> | undefined;
let providers: AiProviderRecord[];
let activeId: string;

async function settle() {
  flushSync();
  await tick();
  flushSync();
}

function button(label: string, root: ParentNode = document) {
  return [...root.querySelectorAll("button")].find((item) => item.textContent?.trim() === label)!;
}

function protocolCard(label: string) {
  return [...document.querySelectorAll<HTMLButtonElement>(".protocol-card")]
    .find((item) => item.querySelector(".protocol-title")?.textContent === label)!;
}

async function setField(selector: string, value: string) {
  const field = document.querySelector<HTMLInputElement>(selector)!;
  field.value = value;
  field.dispatchEvent(new Event("input", { bubbles: true }));
  await settle();
}

async function mountSettings() {
  const target = document.createElement("div");
  document.body.append(target);
  component = mount(AiSettings, { target });
  await vi.waitFor(() => {
    flushSync();
    expect(target.querySelectorAll(".provider-row")).toHaveLength(providers.length);
  });
}

beforeEach(() => {
  setLocale("en");
  activeId = "existing";
  providers = [{
    id: activeId, name: "Existing OpenAI", protocol: "openai-completions",
    endpoint: "https://api.openai.com/v1", model: "gpt-4o", has_api_key: true,
  }];
  invokeMock.mockReset().mockImplementation(async (command: string, args: any) => {
    if (command === "ai_settings_get") {
      return { provider: activeId, danger_mode: false, system_prompt: "" };
    }
    if (command === "ai_settings_set") {
      activeId = args.patch.provider ?? activeId;
      return;
    }
    if (command === "ai_provider_list") return providers.map((provider) => ({ ...provider }));
    if (command === "ai_provider_save") {
      const id = args.patch.id ?? "new-provider";
      const existing = providers.find((provider) => provider.id === id);
      providers = [...providers.filter((provider) => provider.id !== id), {
        ...existing, ...args.patch, id, has_api_key: !!args.patch.apiKey || !!existing?.has_api_key,
      }];
      return id;
    }
    if (command === "ai_list_models") return [{ id: "gpt-6.1-sol" }];
    return [];
  });
});

afterEach(async () => {
  if (component) await unmount(component);
  component = undefined;
  document.body.innerHTML = "";
});

describe("AI provider protocol selection", () => {
  it("keeps Completions as the new-provider default and preserves existing OpenAI configurations", async () => {
    await mountSettings();
    button("New Provider").click();
    await settle();
    expect(protocolCard("OpenAI Completions").getAttribute("aria-pressed")).toBe("true");
    button("Cancel", document.querySelector(".inline-form")!).click();
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="Edit Existing OpenAI"]')!.click();
    await settle();
    expect(protocolCard("OpenAI Completions").getAttribute("aria-pressed")).toBe("true");
    button("Save", document.querySelector(".inline-form")!).click();
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith("ai_provider_save", {
      patch: {
        id: "existing", name: "Existing OpenAI", protocol: "openai-completions",
        endpoint: "https://api.openai.com/v1", model: "gpt-4o", apiKey: undefined,
      },
    }));
  });

  it("loads models and saves Responses only after selecting its peer configuration type", async () => {
    await mountSettings();
    button("New Provider").click();
    await settle();
    const responses = protocolCard("OpenAI Responses");
    expect(responses).toBeDefined();
    expect(responses.previousElementSibling).toBe(protocolCard("OpenAI Completions"));
    responses.click();
    await settle();
    expect(responses.getAttribute("aria-pressed")).toBe("true");
    await setField('.inline-form input[type="text"]', "New Responses");
    button("OpenAI", document.querySelector(".inline-form")!).click();
    await settle();
    await setField('.inline-form input[type="password"]', "test-api-key");
    button("𖦹 Refresh").click();
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith("ai_list_models", {
      providerId: null, protocol: "openai-responses", endpoint: "https://api.openai.com/v1", apiKey: "test-api-key",
    }));
    await settle();
    document.querySelector<HTMLButtonElement>('[aria-label="MODEL"]')!.click();
    await settle();
    button("gpt-6.1-sol").click();
    await settle();
    button("Save", document.querySelector(".inline-form")!).click();
    await vi.waitFor(() => {
      flushSync();
      expect(document.querySelectorAll(".provider-row")).toHaveLength(2);
    });
    expect(invokeMock).toHaveBeenCalledWith("ai_provider_save", {
      patch: {
        id: undefined, name: "New Responses", protocol: "openai-responses",
        endpoint: "https://api.openai.com/v1", model: "gpt-6.1-sol", apiKey: "test-api-key",
      },
    });
    document.querySelector<HTMLButtonElement>('[aria-label="Edit New Responses"]')!.click();
    await settle();
    expect(protocolCard("OpenAI Responses").getAttribute("aria-pressed")).toBe("true");
  });

  it("changes an existing provider protocol explicitly while preserving its endpoint, model and stored key", async () => {
    await mountSettings();
    document.querySelector<HTMLButtonElement>('[aria-label="Edit Existing OpenAI"]')!.click();
    await settle();
    const responses = protocolCard("OpenAI Responses");
    expect(responses).toBeDefined();
    responses.click();
    await settle();
    button("Save", document.querySelector(".inline-form")!).click();
    await vi.waitFor(() => expect(invokeMock).toHaveBeenCalledWith("ai_provider_save", {
      patch: {
        id: "existing", name: "Existing OpenAI", protocol: "openai-responses",
        endpoint: "https://api.openai.com/v1", model: "gpt-4o", apiKey: undefined,
      },
    }));
  });
});
