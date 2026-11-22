<script lang="ts">
  import { onMount, tick } from "svelte";
  import { errCoded, errMsg, t } from "../i18n/index.svelte.ts";
  import { createScopedBackHandler } from "../stores/back.svelte.ts";
  import * as reports from "../stores/incident-reports.svelte.ts";
  import { toast } from "../stores/toast.svelte.ts";
  import { editedText, textareaValue } from "../textarea.ts";
  import AppIcon from "./AppIcon.svelte";

  let report = $state.raw<reports.IncidentReport | null>(null);
  let content = $state("");
  let busy = $state(false);
  let listError = $state("");
  let saveError = $state.raw<unknown>(null);
  let mounted = true;
  const conflict = $derived(errCoded(saveError)?.code === "incident_report_edit_conflict");
  const draftTitle = $derived(content.split("\n", 1)[0].trim());
  const visibleReports = $derived.by(() => {
    const index = reports.reports();
    const id = report?.id;
    if (!id || !draftTitle) return index;
    return index.map(item => item.id === id ? { ...item, title: draftTitle } : item);
  });
  const registerBack = createScopedBackHandler();

  // Keyboard actions replace their own button. If the user has since moved
  // elsewhere, keep that focus instead of pulling them back after IPC.
  async function focusAfterRender(id: string) {
    await tick();
    if (mounted && document.activeElement === document.body) {
      document.getElementById(id)?.focus({ preventScroll: true });
    }
  }

  function cancel(restoreFocus = false) {
    const id = report?.id;
    const retryList = !!saveError;
    report = null;
    content = "";
    saveError = null;
    if (retryList) void refreshList().catch(e => toast.error(errMsg(e)));
    if (restoreFocus && id) void focusAfterRender(`incident-edit-${id}`);
  }

  async function refreshList() {
    await reports.refreshReports();
    listError = "";
    if (report && !reports.reports().some(item => item.id === report?.id)) cancel();
  }

  async function loadList() {
    busy = true;
    try {
      await refreshList();
    } catch (e) {
      listError = errMsg(e);
    } finally {
      busy = false;
    }
  }

  function refreshOnFocus() {
    if (!busy) void refreshList().catch(e => toast.error(errMsg(e)));
  }

  async function edit(id: string, keyboard: boolean) {
    if (busy || conflict) return;
    busy = true;
    try {
      const next = await reports.loadReport(id);
      if (!mounted) return;
      await reports.selectReport(id);
      if (!mounted) return;
      report = next;
      content = next.content;
      saveError = null;
      if (keyboard) void focusAfterRender("incident-markdown");
    } catch (e) {
      toast.error(errMsg(e));
      if (errCoded(e)?.code === "incident_report_not_found") {
        await refreshList().catch(error => toast.error(errMsg(error)));
      }
    } finally {
      busy = false;
    }
  }

  async function save(keyboard: boolean) {
    if (!report || busy || conflict || !draftTitle) return;
    busy = true;
    try {
      await reports.updateReport(report.id, content, report.content);
      cancel(keyboard);
      toast.success(t("report.saved"));
    } catch (e) {
      saveError = e;
      if (!mounted) toast.error(errMsg(e));
    } finally {
      busy = false;
    }
  }

  async function remove(id: string, keyboard: boolean) {
    if (busy || report) return;
    busy = true;
    try {
      const position = reports.reports().findIndex(item => item.id === id);
      await reports.deleteReport(id);
      if (keyboard) {
        const remaining = reports.reports();
        const next = remaining[Math.min(position, remaining.length - 1)];
        void focusAfterRender(next ? `incident-edit-${next.id}` : "incident-empty");
      }
    } catch (e) {
      toast.error(errMsg(e));
    } finally {
      busy = false;
    }
  }

  onMount(() => {
    const unregister = registerBack({
      enabled: () => !!report,
      handle: () => { if (!busy) cancel(true); },
    });
    void loadList();
    return () => { mounted = false; unregister(); };
  });
</script>

<svelte:window onfocus={refreshOnFocus} />

<div class="page">
  {#if listError}
    <p role="alert">{listError}</p>
    <button type="button" class="btn btn-sm" onclick={loadList} disabled={busy}>{t("common.retry")}</button>
  {:else if visibleReports.length}
    <ul role="list" aria-label={t("settings.section.incident_reports")}>
      {#each visibleReports as item (item.id)}
        {@const label = reports.reportLabel(item, visibleReports)}
        {#if report?.id === item.id}
          <li class="card inline-form">
            <div class="item-name" title={label}>{label}</div>
            <label for="incident-markdown">{t("report.content")}</label>
            <textarea id="incident-markdown" rows="12" spellcheck={false}
              value={textareaValue(content)} disabled={busy}
              oninput={event => { content = editedText(content, event.currentTarget.value); }}></textarea>
            {#if saveError}<p role="alert">{errMsg(saveError)}</p>{/if}
            <div class="actions">
              <button type="button" class="btn btn-accent btn-sm" onclick={event => save(event.detail === 0)} disabled={busy || conflict || !draftTitle}>{t("common.save")}</button>
              <button type="button" class="btn btn-sm" onclick={event => cancel(event.detail === 0)} disabled={busy}>{t("common.cancel")}</button>
            </div>
          </li>
        {:else}
          <li class="card item-row">
            <div class="item-info">
              <div class="item-name" title={label}>{label}</div>
            </div>
            <div class="item-actions">
              <button type="button" class="btn btn-sm btn-icon" id={`incident-edit-${item.id}`} title={t("common.edit")}
                aria-label={t("common.edit") + " " + label} onclick={event => edit(item.id, event.detail === 0)} disabled={busy || conflict}>
                <AppIcon name="edit" size={16} />
              </button>
              <button type="button" class="btn btn-sm btn-icon btn-danger" title={t("common.delete")}
                aria-label={t("common.delete") + " " + label} onclick={event => remove(item.id, event.detail === 0)} disabled={busy || !!report}>
                <AppIcon name="trash" size={16} />
              </button>
            </div>
          </li>
        {/if}
      {/each}
    </ul>
  {:else}
    <p class="status" id="incident-empty" tabindex="-1">{busy ? t("common.loading") : t("report.empty")}</p>
  {/if}
</div>

<style>
  .page { padding: 24px; }
  ul { list-style: none; }
  .item-row, .item-actions, .actions { display: flex; align-items: center; gap: 8px; }
  .item-row, .inline-form { margin-bottom: 16px; }
  .item-info { min-width: 0; flex: 1; }
  .item-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 14px; font-weight: 600; }
  .inline-form { display: grid; grid-template-columns: minmax(0, 1fr); gap: 10px; min-width: 0; padding: 16px; }
  .actions { flex-wrap: wrap; }
  textarea {
    width: 100%; min-height: 8em; resize: vertical; line-height: 1.5; font-family: var(--term-font);
    -webkit-user-select: text; user-select: text;
  }
  .status { color: var(--text-sub); }
  [role="alert"] { color: var(--error); }
  @media (max-width: 600px) {
    .page { padding: 12px; }
    textarea { font-size: 16px; }
  }
</style>
