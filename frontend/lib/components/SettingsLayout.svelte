<script lang="ts">
  import { onMount, type Component } from "svelte";
  import * as app from "../stores/app.svelte.ts";
  import * as layout from "../stores/layout.svelte.ts";
  import * as updates from "../stores/updates.svelte.ts";
  import * as syncStatus from "../stores/sync.svelte.ts";
  import * as cliStatus from "../stores/cli.svelte.ts";
  import { createScopedBackHandler, requestBack } from "../stores/back.svelte.ts";
  import { t, locale, setLocale, AVAILABLE_LOCALES, type Locale } from "../i18n/index.svelte.ts";
  import type { MessageKey } from "../i18n/locales/en";
  import CredentialManager from "./CredentialManager.svelte";
  import CredentialEditor from "./CredentialEditor.svelte";
  import DynamicDiscoverySettings from "./DynamicDiscoverySettings.svelte";
  import GroupManager from "./GroupManager.svelte";
  import SnippetManager from "./SnippetManager.svelte";
  import HighlightManager from "./HighlightManager.svelte";
  import SyncScreen from "./SyncScreen.svelte";
  import ImportExportScreen from "./ImportExportScreen.svelte";
  import RecordingSettings from "./RecordingSettings.svelte";
  import PlaybackScreen from "./PlaybackScreen.svelte";
  import ShortcutsScreen from "./ShortcutsScreen.svelte";
  import AboutScreen from "./AboutScreen.svelte";
  import ShellSettings from "./ShellSettings.svelte";
  import CommandBlockSettings from "./CommandBlockSettings.svelte";
  import CliSettings from "./CliSettings.svelte";
  import AppearanceSettings from "./AppearanceSettings.svelte";
  import AiSettings from "./AiSettings.svelte";
  import PluginManager from "../plugins/PluginManager.svelte";
  import Select from "./Select.svelte";
  import ConnectionList from "./ConnectionList.svelte";
  import ConnectionEditor from "./ConnectionEditor.svelte";

  const registerBack = createScopedBackHandler();

  onMount(() => registerBack({
    handle: app.settingsBack,
    enabled: app.settingsCanGoBack,
    priority: 0,
  }));

  /** locale 下拉选项 —— AVAILABLE_LOCALES 是常量，不需要 $derived。 */
  const localeOptions = AVAILABLE_LOCALES.map((l) => ({ value: l.code, label: l.label }));

  type MenuItem = { id: app.SettingsPage; label: string; section: MessageKey };

  type PageRoute = { component: Component<any>; needsId?: boolean };
  const routes: Partial<Record<app.SettingsPage, PageRoute>> = {
    "connections":        { component: ConnectionList },
    "connection-edit":    { component: ConnectionEditor },
    "credentials":        { component: CredentialManager },
    "credential-edit":    { component: CredentialEditor, needsId: true },
    "dynamic-discovery":  { component: DynamicDiscoverySettings },
    "groups":             { component: GroupManager },
    "snippets":           { component: SnippetManager },
    "highlights":         { component: HighlightManager },
    "sync":               { component: SyncScreen },
    "import-export":      { component: ImportExportScreen },
    "shell-settings":     { component: ShellSettings },
    "command-blocks":     { component: CommandBlockSettings },
    "recording-settings": { component: RecordingSettings },
    "playback":           { component: PlaybackScreen },
    "cli":                { component: CliSettings },
    "appearance":         { component: AppearanceSettings },
    "ai":                 { component: AiSettings },
    "plugins":            { component: PluginManager },
    "shortcuts":          { component: ShortcutsScreen },
    "about":              { component: AboutScreen },
  };

  let compact = $derived(layout.compact());

  // 注：菜单数据用 t() 直接调用，配合 $derived 触发响应式更新
  let allMenu = $derived<MenuItem[]>([
    { id: "connections", label: t("settings.section.connection_list"), section: "settings.group.connections" },
    { id: "dynamic-discovery", label: t("settings.section.dynamic_discovery"), section: "settings.group.connections" },
    { id: "credentials", label: t("settings.section.credentials"), section: "settings.group.connections" },
    { id: "groups", label: t("settings.section.groups"), section: "settings.group.connections" },
    { id: "import-export", label: t("settings.section.import_export"), section: "settings.group.connections" },
    { id: "shell-settings", label: t("settings.section.shell"), section: "settings.group.sessions" },
    { id: "recording-settings", label: t("settings.section.recording"), section: "settings.group.sessions" },
    { id: "appearance", label: t("settings.section.appearance"), section: "settings.group.appearance" },
    { id: "highlights", label: t("settings.section.highlights"), section: "settings.group.appearance" },
    { id: "snippets", label: t("settings.section.snippets"), section: "settings.group.appearance" },
    // Advanced: power features beyond a plain terminal (sync, AI, CLI).
    { id: "command-blocks", label: t("settings.section.command_block"), section: "settings.group.advanced" },
    { id: "sync", label: t("settings.section.sync"), section: "settings.group.advanced" },
    { id: "ai", label: t("settings.section.ai"), section: "settings.group.advanced" },
    { id: "plugins", label: t("settings.section.plugins"), section: "settings.group.advanced" },
    { id: "cli", label: t("settings.section.cli"), section: "settings.group.advanced" },
    { id: "shortcuts", label: t("settings.section.shortcuts"), section: "settings.group.help" },
    { id: "about", label: t("settings.section.about"), section: "settings.group.help" },
  ]);

  function pageAvailable(id: app.SettingsPage): boolean {
    const capabilities = app.capabilities();
    switch (id) {
      case "cli": return capabilities.cliInstall;
      case "dynamic-discovery": return capabilities.localDiscovery;
      case "plugins": return capabilities.plugins;
      default: return true;
    }
  }
  let menu = $derived(allMenu.filter(m => pageAvailable(m.id)));

  let sections = $derived((() => {
    const seen = new Set<string>();
    return menu.reduce<{ section: MessageKey; items: MenuItem[] }[]>((acc, m) => {
      if (!seen.has(m.section)) {
        seen.add(m.section);
        acc.push({ section: m.section, items: [] });
      }
      acc[acc.length - 1].items.push(m);
      return acc;
    }, []);
  })());

  function isActive(id: string): boolean {
    const p = app.settingsPage();
    if (p === id) return true;
    if (id === "connections" && p === "connection-edit") return true;
    if (id === "credentials" && p === "credential-edit") return true;
    if (id === "groups" && p === "group-edit") return true;
    return false;
  }

  function hasMenuDot(id: app.SettingsPage): boolean {
    return (id === "about" && updates.hasUpdate())
      || (id === "sync" && syncStatus.anyVersionDifference())
      || (id === "cli" && cliStatus.needsAttention());
  }

  function pageTitle(): string {
    const page = app.settingsPage();
    if (page === "connection-edit") {
      const mode = app.connectionEditorIntent().mode;
      if (mode === "copy") return t("settings.connection_copy");
      return t(mode === "edit" ? "settings.connection_edit" : "settings.connection_create");
    }
    if (page === "credential-edit") {
      return t(app.editingId() ? "settings.credential_edit" : "settings.credential_create");
    }
    if (page === "playback") return t("recording.playback");
    return allMenu.find(item => item.id === page)?.label ?? t("settings.title");
  }
</script>

<div class="settings-layout" class:compact>
  {#if compact}
    <header class="settings-header">
      <button type="button" class="settings-back" onclick={() => requestBack()} aria-label={t("common.back")} title={t("common.back")}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          <path d="m14 6-6 6 6 6" />
        </svg>
      </button>
      <h1 id="settings-page-title">{pageTitle()}</h1>
    </header>
  {/if}
  {#if !compact || app.settingsPage() === "menu"}
  <nav class="settings-menu" aria-label={t("settings.title")}>
    {#if !compact}<div class="menu-header">{t("settings.title")}</div>{/if}
    {#each sections as s}
      <div class="section-label">{t(s.section)}</div>
      {#each s.items as item}
        <button
          class="menu-item surface-raised-sm"
          class:active={isActive(item.id)}
          aria-current={isActive(item.id) ? "page" : undefined}
          onclick={() => app.settingsNavigate(item.id)}
        >
          {item.label}
          {#if hasMenuDot(item.id)}
            <span class="menu-dot" aria-hidden="true"></span>
          {/if}
        </button>
      {/each}
    {/each}
    <div >
      <Select
        value={locale()}
        options={localeOptions}
        onchange={(v) => setLocale(v as Locale)}
        ariaLabel={t("settings.locale.aria")}
      />
    </div>
  </nav>
  {/if}

  {#if !compact || app.settingsPage() !== "menu"}
  <div class="settings-content">
    {#if app.settingsPage() === "menu"}
      <div class="welcome">
        <h2>{t("settings.title")}</h2>
        <p>Select a category from the menu</p>
      </div>
    {:else}
      {@const route = routes[app.settingsPage()]}
      {#if route && pageAvailable(app.settingsPage())}
        {@const C = route.component}
        {#if route.needsId}
          <C id={app.editingId()} />
        {:else}
          <C />
        {/if}
      {/if}
    {/if}
  </div>
  {/if}
</div>

<style>
  .settings-layout {
    display: flex;
    height: 100%;
    min-height: 0;
    min-width: 0;
    overflow: hidden;
  }

  .settings-menu {
    width: 230px;
    flex-shrink: 0;
    background: var(--bg);
    border-right: 1px solid var(--divider);
    padding-top: 12px;
    padding-bottom: 12px;
    padding-left: 18px;
    padding-right: 14px;
    overflow-y: auto;
    min-height: 0;
  }

  .menu-header,
  .settings-header h1 {
    font-size: 16px;
    font-weight: 700;
    color: var(--text);
  }
  .menu-header {
    padding: 4px 0px 4px;
  }

  .settings-menu .section-label {
    padding: 8px 0px 8px;
    font-size: 8px;
  }

  .menu-item {
    display: block;
    width: 100%;
    padding: calc(9px * var(--density)) calc(12px * var(--density));
    margin-bottom: calc(12px * var(--density));
    border: none;
    border-radius: var(--radius-sm);
    background: var(--bg);
    box-shadow: var(--raised-sm);
    color: var(--text-sub);
    font-family: inherit;
    font-size: 13px;
    text-align: left;
    cursor: pointer;
    transition: box-shadow 0.15s, color 0.15s, transform 0.1s;
  }
  .menu-item:hover { box-shadow: var(--raised-sm); color: var(--text); }
  .menu-item:active { box-shadow: var(--pressed); transform: scale(0.99); }
  .menu-item.active {
    box-shadow: var(--pressed);
    color: var(--accent);
    font-weight: 600;
  }
  .menu-dot {
    display: inline-block;
    margin-left: 6px;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--error);
    vertical-align: middle;
  }

  .settings-content {
    flex: 1;
    overflow-y: auto;
    min-width: 0;
    min-height: 0;
  }

  .welcome {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    height: 100%;
    color: var(--text-dim);
    gap: 8px;
  }
  .welcome h2 { font-size: 20px; color: var(--text-sub); }
  .lang-select {
    width: 100%;
    padding: 6px 10px;
    border: 1px solid var(--divider);
    border-radius: var(--radius-sm);
    background: var(--surface);
    color: var(--text);
    font-family: inherit;
    font-size: 13px;
    cursor: pointer;
  }

  /* Width changes only the shell around the same mounted page. */
  .settings-layout.compact {
    flex-direction: column;
  }
  .settings-layout.compact .settings-menu {
    flex: 1;
    width: 100%;
    border-right: none;
  }
  .settings-layout.compact .settings-content {
    width: 100%;
  }
  .settings-header {
    display: flex;
    align-items: center;
    gap: 4px;
    flex-shrink: 0;
    min-height: 44px;
    padding: 6px 16px 6px 8px;
    border-bottom: 1px solid var(--divider);
    background: var(--bg);
  }
  .settings-header h1 {
    min-width: 0;
    margin: 0;
    line-height: 1.4;
    overflow-wrap: anywhere;
  }
  .settings-back {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    width: 32px;
    height: 32px;
    padding: 0;
    border: none;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--accent);
    cursor: pointer;
  }
  .settings-back:hover { background: var(--surface); }
  .settings-back:active { box-shadow: var(--pressed); }
  .settings-back:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
