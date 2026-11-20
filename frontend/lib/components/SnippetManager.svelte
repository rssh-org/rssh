<script lang="ts">
  import { onMount, tick } from "svelte";
  import * as app from "../stores/app.svelte.ts";
  import type { Snippet, SnippetParameter } from "../terminal/snippet-parameters";
  import { toast } from "../stores/toast.svelte.ts";
  import { t, errCoded, errMsg } from "../i18n/index.svelte.ts";
  import AppIcon from "./AppIcon.svelte";

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
  let parameterRows: HTMLDivElement;
  let selectedText = $state("");
  let names = $derived(parameters.map(parameter => parameter.name));
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
    selectedText = "";
    editing = index;
  }

  // Textarea values and selections use LF even when an imported snippet uses CRLF or CR.
  function textareaValue(text: string): string {
    return text.replace(/\r\n?/g, "\n");
  }

  function sourceOffset(text: string, displayOffset: number): number {
    let offset = displayOffset;
    for (let crlf = text.indexOf("\r\n"); crlf >= 0 && crlf < offset; crlf = text.indexOf("\r\n", crlf + 2)) {
      offset++;
    }
    return offset;
  }

  function editedText(previous: string, value: string): string {
    if (!previous.includes("\r")) return value;
    const display = textareaValue(previous);
    if (display === value) return previous;

    // Splice the edit into the source, preserving untouched line endings and
    // literal match texts even when the user edits another part of the command.
    let start = 0;
    while (start < display.length && start < value.length && display[start] === value[start]) start++;
    let oldEnd = display.length;
    let newEnd = value.length;
    while (oldEnd > start && newEnd > start && display[oldEnd - 1] === value[newEnd - 1]) {
      oldEnd--;
      newEnd--;
    }
    return previous.slice(0, sourceOffset(previous, start)) + value.slice(start, newEnd) +
      previous.slice(sourceOffset(previous, oldEnd));
  }

  function readSelection() {
    const { selectionStart, selectionEnd } = textarea;
    selectedText = selectionStart === selectionEnd ? "" :
      command.slice(sourceOffset(command, selectionStart), sourceOffset(command, selectionEnd));
  }

  async function addParameter(text = "") {
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
  <div class="card inline-form">
    <label>
      <span>{t("common.name")}</span>
      <input type="text" bind:value={name} placeholder={t("snippet.name_placeholder")} />
    </label>
    <label>
      <span>{t("snippet.command")}</span>
      <textarea rows="3" spellcheck={false} bind:this={textarea} value={textareaValue(command)}
        onselect={readSelection} onpointerup={readSelection} onkeyup={readSelection}
        oninput={event => { command = editedText(command, event.currentTarget.value); readSelection(); }}
        placeholder={t("snippet.command_placeholder")}></textarea>
    </label>
    {#if selectedText}
      <div class="actions">
        <button class="btn btn-sm" onclick={() => addParameter(selectedText)}>{t("snippet.editor.mark")}</button>
      </div>
    {/if}

    <div class="parameter-header">
      <span>{t("snippet.editor.parameters")}</span>
      <button class="btn btn-sm" onclick={() => addParameter()}>{t("snippet.editor.add")}</button>
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
    .inline-form input, .inline-form textarea { font-size: 16px; }
  }
</style>
