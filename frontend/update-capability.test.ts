import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock, mounted } = vi.hoisted(() => ({
  invokeMock: vi.fn(),
  mounted: [] as Array<() => void>,
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
vi.mock("@tauri-apps/api/app", () => ({ getVersion: async () => "1.0.0" }));
vi.mock("svelte", async (original) => ({
  ...await original<typeof import("svelte")>(),
  onMount: (callback: () => void) => { mounted.push(callback); },
}));
vi.mock("./lib/ipc-shim.ts", () => ({ onRuntimeCapabilitiesChanged: vi.fn() }));
vi.mock("./lib/components/AppShell.svelte", () => ({ default: () => {} }));
vi.mock("./lib/components/WelcomeScreen.svelte", () => ({ default: () => {} }));
vi.mock("./lib/stores/app.svelte.ts", () => ({
  loadProfiles: async () => [],
  loadForwards: async () => [],
}));
vi.mock("./lib/ai/store.svelte.ts", () => ({ settings: () => ({}), loadSettings: vi.fn() }));
vi.mock("./lib/stores/sync.svelte.ts", () => ({
  configurationRevision: () => 0,
  startBackgroundChecks: vi.fn(),
}));
vi.mock("./lib/stores/cli.svelte.ts", () => ({ startBackgroundChecks: vi.fn() }));

beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  mounted.length = 0;
  invokeMock.mockReset();
  vi.stubGlobal("window", {
    innerWidth: 1200,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  });
  vi.stubGlobal("localStorage", { getItem: () => "true", setItem: vi.fn() });
});
afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("release updates follow the RSSH host capability", () => {
  it.each([
    ["iPad browser with a headless host", "Mozilla/5.0 (iPad; CPU OS 18_0)", true],
    ["native iOS", "Mozilla/5.0 (iPad; CPU OS 18_0)", false],
    ["unsupported host with a desktop browser", "Mozilla/5.0 (Macintosh)", false],
  ])("keeps the About action and background checks consistent on %s", async (_name, userAgent, supported) => {
    vi.stubGlobal("navigator", { userAgent, maxTouchPoints: userAgent.includes("iPad") ? 5 : 0 });
    invokeMock.mockResolvedValue({
      releaseUpdateCheck: supported,
      cliInstall: false,
      terminalPolicy: {
        imageStorageLimitMb: 128,
        imagePixelLimit: 16_000_000,
        outputBacklogLimitBytes: 128 * 1024 * 1024,
        gpuRenderDefault: true,
      },
    });
    const runtime = await import("./lib/stores/runtime.svelte.ts");
    await runtime.load();
    const { setLocale, t } = await import("./lib/i18n/index.svelte.ts");
    setLocale("en");
    const { render } = await import("svelte/server");
    const { default: AboutScreen } = await import("./lib/components/AboutScreen.svelte");
    const about = render(AboutScreen).body;
    expect(about.includes(t("about.update.check"))).toBe(supported);

    mounted.length = 0;
    const { default: App } = await import("./App.svelte");
    render(App).body;
    expect(mounted).toHaveLength(1);
    mounted[0]();
    await Promise.resolve();
    expect(vi.getTimerCount()).toBe(supported ? 1 : 0);
    expect(invokeMock).toHaveBeenCalledExactlyOnceWith("get_runtime_capabilities");
  });
});
