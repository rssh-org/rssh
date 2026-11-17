<script lang="ts">
  import { untrack } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import * as auth from "../stores/aws-auth.svelte.ts";
  import { t } from "../i18n/index.svelte.ts";

  let { sourceId, region }: { sourceId: string; region: string } = $props();
  let code = $state("");
  const login = $derived(auth.challenge(sourceId));
  const status = $derived(auth.status(sourceId));
  const ready = $derived(auth.commitRequest(sourceId) !== undefined);
  let previousRegion: string | undefined;

  $effect(() => {
    const id = sourceId;
    code = "";
    untrack(() => { void auth.load(id); });
    return () => {
      void auth.cancel(id).catch(error => console.warn("[aws-signin] form cleanup failed:", error));
    };
  });

  $effect(() => {
    const nextRegion = region.trim();
    if (previousRegion !== undefined && previousRegion !== nextRegion) {
      code = "";
      untrack(() => {
        // Status reads describe the source's existing credentials and do not
        // depend on Region. Only an authorization attempt needs cancellation.
        if (auth.authorizing(sourceId)) {
          void auth.cancel(sourceId).catch(error => auth.setError(sourceId, error));
        }
      });
    }
    previousRegion = nextRegion;
  });

  async function openPage(url: string, id = sourceId) {
    try {
      await invoke("open_external_url", { url });
    } catch (error) {
      auth.setError(id, error);
    }
  }

  async function start() {
    const id = sourceId;
    const challenge = await auth.start(id, region.trim());
    if (challenge) await openPage(challenge.authorization_url, id);
  }

  async function complete() {
    if (await auth.complete(sourceId, code)) code = "";
  }

  async function cancel() {
    code = "";
    try { await auth.cancel(sourceId); } catch (error) { auth.setError(sourceId, error); }
  }

  async function logout() {
    code = "";
    try { await auth.logout(sourceId); } catch (error) { auth.setError(sourceId, error); }
  }
</script>

<section class="signin" aria-label={t("dynamic_discovery.aws_signin")}>
  <p class="signin-hint">{t("dynamic_discovery.aws_signin_hint")}</p>
  {#if login}
    <p class="signin-hint">{t("dynamic_discovery.aws_code_hint")}</p>
    <button type="button" class="btn btn-sm" onclick={() => openPage(login.authorization_url)}>{t("dynamic_discovery.aws_open_signin")}</button>
    <label class="code-field">
      <span>{t("dynamic_discovery.aws_authorization_code")}</span>
      <input type="text" bind:value={code} autocomplete="off" spellcheck="false" autocapitalize="off"
        onkeydown={(event) => { if (event.key === "Enter") { event.preventDefault(); void complete(); } }} />
    </label>
    <div class="actions">
      <button type="button" class="btn btn-accent btn-sm" onclick={complete} disabled={auth.busy(sourceId) || !code.trim()}>{t("dynamic_discovery.aws_complete_signin")}</button>
      <button type="button" class="btn btn-sm" onclick={cancel}>{t("common.cancel")}</button>
    </div>
  {:else if status.authenticated}
    <p class="identity">{t(ready ? "dynamic_discovery.aws_signin_ready" : "dynamic_discovery.aws_signed_in", { account: status.account_id ?? "" })}</p>
    {#if status.arn}<p class="signin-hint identity">{status.arn}</p>{/if}
    {#if ready}
      <p class="signin-hint">{t("dynamic_discovery.aws_save_signin_hint")}</p>
      <button type="button" class="btn btn-sm" onclick={cancel}>{t("dynamic_discovery.aws_discard_signin")}</button>
    {:else}
      <div class="actions">
        <button type="button" class="btn btn-sm" onclick={start} disabled={auth.busy(sourceId) || !region.trim()}>{t("dynamic_discovery.aws_signin_again")}</button>
        <button type="button" class="btn btn-sm" onclick={logout} disabled={auth.busy(sourceId)}>{t("dynamic_discovery.aws_signout")}</button>
      </div>
    {/if}
  {:else}
    <p class="signin-hint">{t("dynamic_discovery.aws_signed_out")}</p>
    <button type="button" class="btn btn-accent btn-sm" onclick={start} disabled={auth.busy(sourceId) || !region.trim()}>
      {t(auth.busy(sourceId) ? "common.loading" : "dynamic_discovery.aws_signin")}
    </button>
  {/if}
</section>

<style>
  .signin { display: flex; flex-direction: column; align-items: flex-start; gap: 10px; padding: 12px; border: 1px solid var(--divider); border-radius: var(--radius-sm); min-width: 0; }
  .signin-hint { margin: 0; color: var(--text-dim); font-size: 12px; line-height: 1.6; }
  .identity { margin: 0; overflow-wrap: anywhere; max-width: 100%; }
  .code-field { display: flex; flex-direction: column; gap: 6px; width: 100%; min-width: 0; font-size: 12px; }
  .code-field input { width: 100%; min-width: 0; box-sizing: border-box; }
  .actions { display: flex; flex-wrap: wrap; gap: 8px; }
</style>
