<script lang="ts">
  import { onMount, tick } from "svelte";
  import * as app from "../stores/app.svelte.ts";
  import { snippetPreview, type Snippet, type SnippetParameter } from "../terminal/snippet-parameters";
  import { toast } from "../stores/toast.svelte.ts";
  import { t, errCoded, errMsg } from "../i18n/index.svelte.ts";
  import AppIcon from "./AppIcon.svelte";
  import { editedText, sourceOffset, textareaValue } from "../textarea.ts";

  let snippets = $state<Snippet[]>([]);
  let ready = $state(false);
  let writing = $state(false);
  let loadError = $state("");
  let conflict = $state("");

  let editing = $state<number | "new" | null>(null);
  let name = $state("");
  let command = $state("");
  let parameters = $state<SnippetParameter[]>([]);
  let textarea: HTMLTextAreaElement;
  let highlights: HTMLSpanElement;
  let addButton: HTMLButtonElement;
  let parameterRows: HTMLDivElement;
  let useCommandSelection = false;
  let names = $derived(parameters.map(parameter => parameter.name));
  let commandParts = $derived.by(() => {
    const parts = snippetPreview({ name, command, parameters });
    return parts.map((part, index) => {
      // The textarea shows one newline even if a match boundary splits a CRLF pair.
      const splitCRLF = part.text.endsWith("\r") && parts[index + 1]?.text.startsWith("\n");
      return { ...part, text: textareaValue(splitCRLF ? part.text.slice(0, -1) : part.text) };
    });
  });
  let duplicate = $derived(new Set(names).size !== names.length);
  let canSave = $derived(ready && !writing && !conflict && name.trim() && command.trim() &&
    !duplicate && names.every(name => name.length > 0));

  onMount(refresh);

  async function refresh() {
    ready = false;
    loadError = "";
    try {
      snippets = await app.loadSnippets();
      ready = true;
      conflict = "";
      editing = null;
    } catch (error) {
      loadError = errMsg(error);
    }
  }

  function startEdit(index: number | "new") {
    const original = index === "new" ? undefined : snippets[index];
    name = original?.name ?? "";
    command = original?.command ?? "";
    parameters = (original?.parameters ?? []).map(parameter => ({ ...parameter }));
    useCommandSelection = false;
    editing = index;
  }

  function syncHighlightScroll() {
    highlights.scrollTop = textarea.scrollTop;
    highlights.scrollLeft = textarea.scrollLeft;
  }

  async function addParameter() {
    const { selectionStart, selectionEnd } = textarea;
    const text = useCommandSelection && selectionStart !== selectionEnd
      ? command.slice(sourceOffset(command, selectionStart), sourceOffset(command, selectionEnd))
      : "";
    let index = parameters.findIndex(parameter => parameter.name === text);
    if (index < 0) {
      index = parameters.length;
      parameters.push({ name: text, defaultValue: null });
    }
    await tick();
    const row = parameterRows.children[index];
    const input = row?.querySelector<HTMLTextAreaElement>(text ? ".parameter-default" : ".parameter-name");
    input?.focus();
    input?.select();
  }

  async function update(snippet: Snippet | null, index?: number) {
    if (!ready || writing || conflict) return;
    writing = true;
    try {
      const target = index === undefined ? undefined : { index, previous: snippets[index] };
      snippets = await app.updateSnippet(snippet, target);
      editing = null;
    } catch (error) {
      if (errCoded(error)?.code === "snippet_changed") conflict = errMsg(error);
      else toast.error(errMsg(error));
    } finally {
      writing = false;
    }
  }

  function save() {
    if (!canSave || editing === null) return;
    void update({ name: name.trim(), command, ...(parameters.length ? { parameters } : {}) },
      editing === "new" ? undefined : editing);
  }
</script>

{#snippet editor()}
  <div class="card inline-form" onfocusin={event => {
    if (event.target !== addButton) useCommandSelection = event.target === textarea;
  }}>
    <label>
      <span>{t("common.name")}</span>
      <input type="text" bind:value={name} placeholder={t("snippet.name_placeholder")} />
    </label>
    <label>
      <span>{t("snippet.command")}</span>
      <span class="command-editor">
        <textarea rows="3" spellcheck={false} bind:this={textarea} value={textareaValue(command)}
          onscroll={syncHighlightScroll}
          oninput={event => { command = editedText(command, event.currentTarget.value); }}
          placeholder={t("snippet.command_placeholder")}></textarea>
        <span class="command-highlights" aria-hidden="true" bind:this={highlights}>{#each commandParts as part}<span class:parameter-match={!!part.parameterName}>{part.text}</span>{/each}</span>
      </span>
    </label>
    <div class="parameter-header">
      <span>{t("snippet.editor.parameters")}</span>
      <button class="btn btn-sm" bind:this={addButton} onclick={addParameter}>{t("snippet.editor.add")}</button>
    </div>
    <p class="hint">{t("snippet.editor.hint")}</p>
    <div class="parameter-rows" bind:this={parameterRows}>
      {#each parameters as parameter, index (index)}
        <div class="parameter-row">
          <label>
            <span>{t("snippet.editor.name")}</span>
            <textarea class="parameter-name" rows="1" spellcheck={false} value={textareaValue(parameter.name)}
              oninput={event => { parameter.name = editedText(parameter.name, event.currentTarget.value); }}></textarea>
          </label>
          <label>
            <span>{t("snippet.editor.default")}</span>
            <textarea class="parameter-default" rows="1" spellcheck={false} value={textareaValue(parameter.defaultValue ?? "")}
              oninput={event => { parameter.defaultValue = editedText(parameter.defaultValue ?? "", event.currentTarget.value) || null; }}
              placeholder={t("snippet.editor.required_hint")}></textarea>
          </label>
          <button class="btn btn-sm btn-icon" title={t("common.delete")}
            aria-label={t("common.delete") + " · " + (parameter.name || t("snippet.editor.name"))}
            onclick={() => parameters.splice(index, 1)}><AppIcon name="trash" size={16} /></button>
        </div>
      {/each}
    </div>
    {#if duplicate}<span class="hint" role="alert">{t("snippet.editor.duplicate")}</span>{/if}
    <div class="actions">
      <button class="btn btn-accent btn-sm" disabled={!canSave} onclick={save}>{t("common.save")}</button>
      <button class="btn btn-sm" disabled={writing} onclick={() => { editing = null; }}>{t("common.cancel")}</button>
    </div>
  </div>
{/snippet}

<div class="page">
  <div class="toolbar">
    <button class="btn btn-accent btn-sm" onclick={() => startEdit("new")} disabled={!ready || writing || !!conflict}>{t("snippet.new")}</button>
  </div>
  {#if loadError || conflict}
    <div class="actions" role="alert">
      <span>{loadError || conflict}</span>
      <button class="btn btn-sm" onclick={refresh} disabled={!ready && !loadError}>
        {conflict ? t("snippet.editor.reload") : t("common.retry")}
      </button>
    </div>
  {/if}
  {#if editing === "new"}{@render editor()}{/if}
  {#each snippets as s, i (i)}
    {#if editing === i}
      {@render editor()}
    {:else}
      <div class="card item-row">
        <div class="item-info">
          <div class="item-name" title={s.name}>{s.name}</div>
          <div class="item-sub" title={s.command}>{s.command}</div>
        </div>
        <div class="item-actions">
          <button class="btn btn-sm btn-icon" title={t("common.edit")} aria-label={t("common.edit") + " " + s.name}
            onclick={() => startEdit(i)} disabled={writing || !!conflict}><AppIcon name="edit" size={16} /></button>
          <button class="btn btn-sm btn-icon btn-danger" title={t("common.delete")} aria-label={t("common.delete") + " " + s.name}
            onclick={() => update(null, i)} disabled={writing || !!conflict || editing !== null}><AppIcon name="trash" size={16} /></button>
        </div>
      </div>
    {/if}
  {:else}
    {#if !ready && !loadError}
      <p class="empty">{t("common.loading")}</p>
    {:else if ready && editing !== "new"}
      <p class="empty">{t("snippet.empty")}</p>
    {/if}
  {/each}
</div>

<style>
  .page { padding: 24px; }
  .toolbar, .item-row, .item-actions, .actions, .parameter-header { display: flex; align-items: center; gap: 8px; }
  .toolbar { justify-content: flex-end; margin-bottom: 16px; }
  .item-row, .inline-form { margin-bottom: 16px; }
  .item-info { min-width: 0; flex: 1; }
  .item-name, .item-sub { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .item-name { font-weight: 600; font-size: 14px; }
  .item-sub { font: 12px var(--term-font); color: var(--text-sub); margin-top: 2px; }
  .inline-form, .parameter-rows { display: grid; gap: 10px; }
  .inline-form { min-width: 0; padding: 16px; }
  .inline-form label { display: grid; gap: 4px; min-width: 0; }
  .inline-form input, .inline-form textarea { width: 100%; min-width: 0; }
  textarea {
    font-family: var(--term-font); text-transform: none; letter-spacing: normal;
    resize: vertical; -webkit-user-select: text; user-select: text;
  }
  .command-editor {
    display: block; position: relative; min-width: 0;
    font: 400 13px/1.5 var(--term-font); text-transform: none; letter-spacing: normal;
  }
  .command-editor textarea, .command-highlights {
    display: block; font: inherit; padding: calc(10px * var(--density)) calc(14px * var(--density));
    white-space: pre-wrap; overflow-wrap: break-word; scrollbar-gutter: stable;
  }
  .command-editor textarea { overflow-y: auto; }
  /* Only the underline is painted here. The native control owns text, selection and the caret. */
  .command-highlights {
    position: absolute; inset: 0; overflow: hidden; border: 1.5px solid transparent;
    color: transparent; pointer-events: none; user-select: none; forced-color-adjust: none;
  }
  .command-highlights::after { content: "\200b"; } /* Keep a final empty line scrollable. */
  .parameter-match { text-decoration: underline 1px var(--accent); text-underline-offset: 2px; text-decoration-skip-ink: none; }
  @media (forced-colors: active) {
    .parameter-match { text-decoration-color: Highlight; }
  }
  .parameter-header { justify-content: space-between; }
  .parameter-row { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) auto; gap: 8px; align-items: end; }
  .parameter-rows:empty { display: none; }
  .hint, .empty { color: var(--text-sub); font-size: 12px; }
  .empty { padding: 32px; text-align: center; }
  .actions { flex-wrap: wrap; }
  @media (max-width: 600px) {
    .page { padding: 12px; }
    .parameter-row { grid-template-columns: minmax(0, 1fr) auto; }
    .parameter-row > label { grid-column: 1; }
    .parameter-row > button { grid-row: 1; grid-column: 2; }
    .inline-form input, .inline-form textarea, .command-editor { font-size: 16px; }
  }
</style>
