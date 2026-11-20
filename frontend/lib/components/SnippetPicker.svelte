<script lang="ts">
  import { flushSync, onDestroy, onMount } from "svelte";
  import { SvelteMap } from "svelte/reactivity";
  import { t, errMsg } from "../i18n/index.svelte.ts";
  import * as app from "../stores/app.svelte.ts";
  import type { Snippet } from "../stores/app.svelte.ts";
  import { createScopedBackHandler } from "../stores/back.svelte.ts";
  import { toast } from "../stores/toast.svelte.ts";
  import { snippetPreview } from "../terminal/snippet-parameters";
  import AppIcon from "./AppIcon.svelte";

  const registerBack = createScopedBackHandler();
  const targetId = app.snippetPickerTargetId();
  const values = new SvelteMap<string, string>();

  let snippets = $state.raw<Snippet[]>([]);
  let selected = $state.raw<Snippet | null>(null);
  let activeParameter = $state("");
  let focusIdx = $state(0);
  let filter = $state("");
  let searchInput: HTMLInputElement;
  let list: HTMLDivElement;
  let fields = $state<HTMLDivElement>();
  let dialog: HTMLDivElement;
  let viewportTop = $state(0);
  let viewportHeight = $state<number>();
  let targetTab = $derived(app.tabs().find(tab => tab.id === targetId));
  let parameters = $derived(selected?.parameters ?? []);
  let preview = $derived(selected ? snippetPreview(selected, values) : []);
  let complete = $derived(parameters.every(({ name }) => !!values.get(name)));
  let filtered = $derived(snippets.filter(snippet =>
    snippet.name.toLowerCase().includes(filter.toLowerCase()) ||
    snippet.command.toLowerCase().includes(filter.toLowerCase())
  ));

  onMount(() => {
    void app.loadSnippets().then(result => { snippets = result; }).catch(error => toast.error(errMsg(error)));
    searchInput.focus();
    return registerBack({ handle: back, enabled: app.snippetPickerOpen });
  });
  onDestroy(() => app.terminalFocus(app.activeTabId()));

  onMount(() => {
    // Overlay keyboards shrink the visual viewport while a fixed dialog can
    // still occupy the full layout viewport. Keep the footer in the visible area.
    const viewport = window.visualViewport;
    if (!viewport) return;
    const update = () => {
      viewportTop = viewport.offsetTop;
      viewportHeight = viewport.height;
    };
    update();
    viewport.addEventListener("resize", update);
    viewport.addEventListener("scroll", update);
    return () => {
      viewport.removeEventListener("resize", update);
      viewport.removeEventListener("scroll", update);
    };
  });

  function send(text: string) {
    if (targetId && app.sendSnippetToTerminal(targetId, text)) app.closeSnippetPicker();
    else toast.error(t("snippet.picker.target_unavailable"));
  }

  function choose(snippet: Snippet, index: number) {
    focusIdx = index;
    if (!snippet.parameters?.length) {
      send(snippet.command);
      return;
    }
    selected = snippet;
    values.clear();
    for (const { name, defaultValue } of snippet.parameters) values.set(name, defaultValue ?? "");
    flushSync();
    fields?.querySelector("textarea")?.focus();
  }

  function fieldLabel(name: string) {
    return name.trim() !== name || /[\r\n\t]| {2,}/.test(name) ? JSON.stringify(name) : name;
  }

  function back() {
    if (!selected) {
      app.closeSnippetPicker();
      return;
    }
    selected = null;
    flushSync();
    searchInput.focus();
  }

  function submit() {
    const missing = parameters.findIndex(({ name }) => !values.get(name));
    if (missing >= 0) {
      fields?.querySelectorAll("textarea")[missing]?.focus();
      return;
    }
    send(preview.map(part => part.text).join(""));
  }

  function handleKey(event: KeyboardEvent) {
    if (event.isComposing || event.keyCode === 229) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      back();
      return;
    }
    if (event.key === "Tab") {
      // Keep keyboard navigation inside the currently visible dialog step.
      const step = selected ? ".parameter-step" : ".catalog";
      const controls = dialog.querySelector(step)!.querySelectorAll<HTMLElement>("button:enabled, input, textarea");
      const first = controls[0], last = controls[controls.length - 1];
      if (event.shiftKey ? event.target === first : event.target === last) {
        event.preventDefault();
        (event.shiftKey ? last : first)?.focus();
      }
      return;
    }
    if (event.key === "Enter") {
      if (event.repeat) { event.preventDefault(); return; }
      if (event.target === searchInput && filtered[focusIdx]) {
        event.preventDefault();
        choose(filtered[focusIdx], focusIdx);
      } else if (selected && event.target instanceof HTMLTextAreaElement && !event.shiftKey) {
        event.preventDefault();
        submit();
      }
      return;
    }
    if (event.target !== searchInput || !filtered.length) return;
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const direction = event.key === "ArrowDown" ? 1 : -1;
    focusIdx = Math.max(0, Math.min(focusIdx + direction, filtered.length - 1));
    list.children[focusIdx]?.scrollIntoView({ block: "nearest" });
  }
</script>

<div class="picker-backdrop" role="presentation"
  style:top={viewportTop + "px"} style:height={viewportHeight == null ? undefined : viewportHeight + "px"}
  onclick={(event) => { if (event.target === event.currentTarget) app.closeSnippetPicker(); }}>
  <div class="picker surface-popup" role="dialog" aria-modal="true" tabindex="-1"
    aria-label={t("settings.section.snippets")} bind:this={dialog} onkeydown={handleKey}>
    <div class="catalog" hidden={!!selected}>
      <input class="search" bind:this={searchInput} type="text" autocomplete="off" spellcheck={false}
        aria-label={t("snippet.picker.search")} placeholder={t("snippet.picker.search")} bind:value={filter}
        oninput={() => { focusIdx = 0; list.scrollTop = 0; }} />
      <div class="picker-list" bind:this={list}>
        {#each filtered as snippet, index (snippet)}
          <button class="picker-item" class:focused={focusIdx === index}
            onfocus={() => { focusIdx = index; }} onclick={() => choose(snippet, index)}>
            <span class="picker-name">{snippet.name}</span>
            <code class="picker-cmd">{#each snippetPreview(snippet) as part}<span class:marked={!!part.parameterName}>{part.text}</span>{/each}</code>
          </button>
        {:else}
          <div class="picker-empty">{t(filter ? "snippet.picker.no_matches" : "snippet.empty")}</div>
        {/each}
      </div>
    </div>
    {#if selected}
      <div class="parameter-step">
        <header>
          <button class="btn btn-sm back" onclick={back}>
            <svg viewBox="0 0 16 16" width="14" height="14" fill="none" aria-hidden="true">
              <path d="m10 3-5 5 5 5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>
            </svg>{t("common.back")}
          </button>
          <h2>{selected.name}</h2>
        </header>
        <div class="parameter-content">
          <pre aria-label={t("snippet.command")}><code>{#each preview as part}<span class:active={!!part.parameterName && part.parameterName === activeParameter} class:empty={!!part.parameterName && !part.text}>{part.text || part.parameterName || ""}</span>{/each}</code></pre>
          <div class="fields" bind:this={fields}>
            {#each parameters as { name } (name)}
              <label>
                <span>{fieldLabel(name)}</span>
                <textarea rows="1" required autocomplete="off" autocapitalize="off" spellcheck={false}
                  value={values.get(name) ?? ""} enterkeyhint="done"
                  onfocus={(event) => { activeParameter = name; event.currentTarget.select(); }}
                  oninput={(event) => values.set(name, event.currentTarget.value)}></textarea>
              </label>
            {/each}
          </div>
        </div>
        <footer>
          {#if targetTab}
            <span class="target" title={t("snippet.picker.target", { name: app.tabLabel(targetTab) })}>
              <AppIcon name="terminal" size={14} />
              <span>{app.tabLabel(targetTab)}</span>
            </span>
          {/if}
          <button class="btn btn-accent" disabled={!complete} onclick={submit}>{t("snippet.picker.send")}</button>
        </footer>
      </div>
    {/if}
  </div>
</div>

<style>
  .picker-backdrop {
    position: fixed; inset: 0; z-index: 400;
    background: var(--overlay-soft);
    display: flex; align-items: flex-start; justify-content: center;
    padding: min(80px, 10dvh) 16px 16px;
  }
  .picker {
    width: min(100%, 440px); max-height: 100%;
    display: flex; flex-direction: column; overflow: hidden;
  }
  .catalog:not([hidden]), .parameter-step { display: flex; flex-direction: column; min-height: 0; }
  .catalog { max-height: 360px; }
  .search { margin: 12px; flex-shrink: 0; }
  .picker-list { min-height: 0; overflow-y: auto; padding: 0 8px 8px; }
  .picker-item {
    display: flex; flex-direction: column; gap: 2px;
    width: 100%; padding: 8px 12px; border: none;
    border-radius: var(--radius-sm);
    background: transparent; text-align: left;
    font-family: inherit; color: var(--text); cursor: pointer;
  }
  .picker-item:hover, .picker-item.focused { background: color-mix(in srgb, var(--text) 8%, transparent); }
  .picker-item.focused { outline: 1px solid var(--accent); outline-offset: -1px; }
  .picker-name, h2 { font-size: 14px; font-weight: 600; overflow-wrap: anywhere; }
  .picker-cmd {
    display: block; max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
    font: 12px/1.5 var(--term-font); color: var(--text-sub);
  }
  .marked { text-decoration: underline var(--text-dim); text-underline-offset: 3px; }
  .picker-empty { padding: 16px; text-align: center; color: var(--text-sub); }
  header, footer { flex-shrink: 0; display: flex; align-items: center; gap: 12px; padding: 12px 16px; }
  header { border-bottom: 1px solid var(--divider); }
  h2 { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .back { gap: 4px; }
  .parameter-content { min-height: 0; overflow: auto; padding: 16px; }
  pre {
    margin: 0 0 16px; padding: 12px; border-radius: var(--radius-sm);
    background: var(--term-bg); color: var(--term-fg); white-space: pre-wrap; overflow-wrap: anywhere;
    user-select: text; -webkit-user-select: text;
  }
  pre code { font: 13px/1.65 var(--term-font); }
  pre .active { text-decoration: underline 2px var(--accent); text-underline-offset: 3px; }
  pre .empty { color: var(--text-sub); text-decoration: underline dotted; text-underline-offset: 3px; }
  .fields { display: grid; gap: 12px; }
  .fields label {
    min-width: 0; display: grid; gap: 4px; color: var(--text-sub); overflow-wrap: anywhere;
    font: inherit; text-transform: none; letter-spacing: normal;
  }
  .fields label > span { white-space: pre-wrap; }
  .fields textarea { min-width: 0; width: 100%; resize: vertical; font-family: var(--term-font); }
  footer { border-top: 1px solid var(--divider); justify-content: flex-end; }
  .target { min-width: 0; margin-right: auto; display: flex; align-items: center; gap: 6px; color: var(--text-sub); }
  .target span { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  footer button, .back { flex-shrink: 0; }
  @media (max-width: 639px) {
    .picker-backdrop {
      padding: max(8px, var(--app-inset-top, env(safe-area-inset-top, 0px)))
        max(8px, env(safe-area-inset-right, 0px)) max(8px, env(safe-area-inset-bottom, 0px))
        max(8px, env(safe-area-inset-left, 0px));
    }
    .search, .fields textarea { font-size: 16px; }
    .picker-item, .back, footer button { min-height: 44px; }
    header, footer { padding: 8px 12px; }
  }
</style>
