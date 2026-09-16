<script lang="ts">
  import { onMount } from "svelte";
  import AppShell from "./lib/components/AppShell.svelte";
  import ToastStack from "./lib/components/ToastStack.svelte";
  import WelcomeScreen from "./lib/components/WelcomeScreen.svelte";
  import { isIOS, loadProfiles, loadForwards } from "./lib/stores/app.svelte.ts";
  import * as updates from "./lib/stores/updates.svelte.ts";
  import * as sync from "./lib/stores/sync.svelte.ts";
  import * as cli from "./lib/stores/cli.svelte.ts";
  import * as ai from "./lib/ai/store.svelte.ts";
  import * as runtime from "./lib/stores/runtime.svelte.ts";
  import { errMsg, t } from "./lib/i18n/index.svelte.ts";

  // First-launch auto-show: when there are no profiles and no forwards,
  // surface the cinematic welcome once. After the user dismisses it
  // (Get Started / Skip / Esc / Close) we set this localStorage flag
  // so a later "wiped everything" state doesn't loop the demo on every
  // boot. Settings → About → "Preview welcome screen" is the manual
  // replay path; it deliberately bypasses this flag.
  const DISMISSED_KEY = "rssh.welcome.dismissed";
  // Capture before the capability await: AppShell consumes these handoffs
  // when it mounts, before startup's remaining asynchronous work settles.
  const auxiliaryWindow = !!window.__rssh_clone || !!window.__rssh_ai_handoff;

  let showWelcome = $state(false);
  let startupError = $state("");

  $effect(() => {
    const revision = sync.configurationRevision();
    if (revision === 0) return;
    void ai.loadSettings().catch((e) =>
      console.warn("[sync] AI settings refresh failed:", e),
    );
  });

  onMount(() => { void initialize(); });

  async function initialize() {
    startupError = "";
    try {
      await runtime.load();
    } catch (error) {
      startupError = errMsg(error);
      return;
    }
    // 预热 AI 设置：色条"发送到 AI"等入口靠 ai.settings()?.has_api_key 同步判断是否
    // 可用，过去只有打开 AI 面板才加载它。在 app 启动时拉一次，保证任何菜单打开前
    // _settings 已就位（fire-and-forget；失败不阻塞 UI，开面板时还会重试）。
    // 预热 AI 设置，让"发送到 AI"的置灰判断在任何菜单打开前就确定。
    // 关键：ai_settings_get 只读 DB 偏好 + 查 key 是否存在（has_api_key 走
    // secret_store.exists，不解密），**绝不加载 master key**，所以开机不会弹钥匙串
    // ——钥匙串只在真正发起 LLM 请求（解密 key）时申请。失败不弹 toast（开机噪音），
    // 但按"不静默吞异常"留一行 warn；开 AI 面板时还会重试。
    if (!ai.settings()) {
      void ai.loadSettings().catch((e) => console.warn("[ai] settings preheat failed:", e));
    }

    // Skip background update polling on clone / AI-handoff windows —
    // they're transient and the main window already owns the timer.
    if (!auxiliaryWindow) {
      // App Store/TestFlight owns updates on iOS; never direct those users to a
      // GitHub desktop/Android artifact.
      if (!isIOS) updates.startBackgroundChecks();
      sync.startBackgroundChecks();
      if (runtime.capabilities().cliInstall) cli.startBackgroundChecks();
    }

    // A clone or analysis handoff should open directly into its requested task.
    if (auxiliaryWindow) return;

    // localStorage / Tauri may not be available in non-app hosts
    // (e.g. vitest, browser preview) — defensively swallow errors so a
    // missing API never blocks the regular AppShell from rendering.
    let dismissed = false;
    try {
      dismissed = localStorage.getItem(DISMISSED_KEY) === "true";
    } catch {}
    if (dismissed) return;

    try {
      const [profiles, forwards] = await Promise.all([
        loadProfiles(),
        loadForwards(),
      ]);
      if (profiles.length === 0 && forwards.length === 0) {
        showWelcome = true;
      }
    } catch (e) {
      console.debug("welcome auto-check skipped:", e);
    }
  }

  function dismissWelcome() {
    showWelcome = false;
    try {
      localStorage.setItem(DISMISSED_KEY, "true");
    } catch {}
  }
</script>

{#if runtime.loaded()}
  <AppShell />
{:else}
  <main class="startup" aria-busy={!startupError}>
    {#if startupError}
      <h1>{t("startup.failed")}</h1>
      <p role="alert">{startupError}</p>
      <button class="btn btn-accent" onclick={() => void initialize()}>{t("common.retry")}</button>
    {:else}
      <p role="status">{t("common.loading")}</p>
    {/if}
  </main>
{/if}
<ToastStack />

{#if showWelcome}
  <WelcomeScreen onDismiss={dismissWelcome} />
{/if}

<style>
  .startup { height: 100%; min-height: 0; display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 12px; padding: 24px; text-align: center; }
  .startup h1 { font-size: 18px; }
  .startup p { max-width: 640px; overflow-wrap: anywhere; color: var(--text-sub); }
</style>
