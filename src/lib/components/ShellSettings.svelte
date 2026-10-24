<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import * as app from "../stores/app.svelte.ts";
  import * as startup from "../stores/startup.svelte.ts";
  import type { StartupAction } from "../stores/startup.svelte.ts";
  import * as transfers from "../stores/transfers.svelte.ts";
  import { isMac } from "../stores/keymap.svelte.ts";
  import { t, errMsg } from "../i18n/index.svelte.ts";
  import { toast } from "../stores/toast.svelte.ts";
  import Select from "./Select.svelte";
  import Modal from "./Modal.svelte";

  let shells = $state<string[]>([]);
  let selectedShell = $state("");
  /** Custom radio 自己的 path —— 独立持有，切到内置 shell 再回来不丢用户输入。 */
  let customPath = $state("");
  /** 用户选了 Custom 但还没填路径的瞬态标记 —— 不持久化。
   *  避免把占位字符串写进 local_shell 设置导致重启坏 shell。 */
  let pendingCustom = $state(false);
  let startupAction = $state<StartupAction | null>(null);
  let startupBusy = $state(false);
  let verboseLog = $state(true);
  let connectTimeout = $state(10);
  let copyOnSelect = $state(false);
  let confirmCloseTab = $state(false);
  let rightClickAction = $state<app.RightClickAction>("menu");
  let ctrlRightClickMenu = $state(false);
  /** Modifier key label for the ctrl+right-click hint — Cmd on macOS, Ctrl elsewhere. */
  const modKey = isMac ? "⌘" : "Ctrl";
  let rightClickOptions = $derived([
    { value: "menu", label: t("settings.shell.right_click_menu") },
    { value: "paste", label: t("settings.shell.right_click_paste") },
    { value: "copyPaste", label: t("settings.shell.right_click_copy_paste") },
  ]);
  let startupOptions = $derived([
    { value: "restore", label: t("settings.shell.startup_restore") },
    ...(shells.length ? [{ value: "local", label: t("settings.shell.startup_local") }] : []),
    { value: "home", label: t("settings.shell.startup_home") },
  ]);
  /** SFTP 并发上限。main.ts 启动时 loadMaxConcurrent 已写过 store，
   *  这里 onMount 再读一次显示当前值。 */
  let sftpMaxConcurrent = $state(transfers.maxConcurrent());
  const sftpBounds = transfers.maxConcurrentBounds();

  /** 用户当前选中的是 Custom 还是某个内置 shell。
   *  pendingCustom（点了 Custom 没填）或 selectedShell 不在 shells 里都算 custom。 */
  let customMode = $derived(pendingCustom || (selectedShell !== "" && !shells.includes(selectedShell)));

  onMount(async () => {
    await loadStartup();
    if (app.capabilities().localPty) {
      selectedShell = await invoke<string | null>("get_setting", { key: "local_shell" }) ?? "";
      if (selectedShell && !shells.includes(selectedShell)) {
        customPath = selectedShell;
      }
    }
    verboseLog = (await invoke<string | null>("get_setting", { key: "verbose_log" })) !== "false";
    const ts = await invoke<string | null>("get_setting", { key: "connect_timeout" });
    if (ts) connectTimeout = parseInt(ts, 10) || 10;
    copyOnSelect = await app.loadCopyOnSelect();
    confirmCloseTab = await app.loadConfirmCloseTab();
    rightClickAction = await app.loadRightClickAction();
    ctrlRightClickMenu = await app.loadCtrlRightClickMenu();
    // SFTP 并发上限：main.ts 启动时已读过持久值进 store，但用户可能在打开 Settings 前
    // 还没 await 完。再读一次确保 input 显示真实当前值。
    await transfers.loadMaxConcurrent();
    sftpMaxConcurrent = transfers.maxConcurrent();
  });

  async function saveShell() {
    await invoke("set_setting", { key: "local_shell", value: selectedShell });
  }

  /** 选中内置 shell —— radio onchange 触发。 */
  function pickShell(sh: string) {
    pendingCustom = false;
    selectedShell = sh;
    saveShell();
  }

  /** 切到 Custom radio：仅在 customPath 已有值时才写入持久化；
   *  没填路径时只切 UI 状态，保留之前 selectedShell 不动，避免存空/占位污染。
   *  幂等：input refocus 时也调用本函数，已经等于 selectedShell 就不重复 invoke。 */
  function pickCustom() {
    pendingCustom = true;
    const v = customPath.trim();
    if (v && v !== selectedShell) {
      selectedShell = v;
      saveShell();
    }
  }

  /** Custom input blur：把 input 内容写回 selectedShell。 */
  function onCustomBlur() {
    const v = customPath.trim();
    if (v && pendingCustom) {
      selectedShell = v;
      saveShell();
    }
  }

  async function saveVerbose() {
    await invoke("set_setting", { key: "verbose_log", value: String(verboseLog) });
  }

  async function loadStartup() {
    if (startupBusy) return;
    startupBusy = true;
    try {
      const [action, availableShells] = await Promise.all([startup.action(), startup.availableShells()]);
      shells = availableShells;
      startupAction = action === "local" && !shells.length ? "home" : action;
    } catch (error) {
      startupAction = null;
      toast.error(errMsg(error));
    } finally {
      startupBusy = false;
    }
  }

  async function saveStartup(action: StartupAction) {
    startupBusy = true;
    try {
      await startup.setAction(action);
    } catch (error) {
      toast.error(errMsg(error));
    } finally {
      startupBusy = false;
      await loadStartup();
    }
  }

  async function saveTimeout() {
    const val = Math.max(1, Math.min(300, connectTimeout));
    connectTimeout = val;
    await invoke("set_setting", { key: "connect_timeout", value: String(val) });
  }

  async function saveCopyOnSelect() {
    await app.setCopyOnSelect(copyOnSelect);
  }

  async function saveConfirmCloseTab() {
    await app.setConfirmCloseTab(confirmCloseTab);
  }

  /** Right-click change awaiting confirmation — non-null opens the dialog.
   *  Only leaving "menu" needs one: that transition hides the system menu
   *  (the copy/paste entry point); every other switch changes nothing visible. */
  let pendingRightClick = $state<app.RightClickAction | null>(null);

  function saveRightClickAction(v: app.RightClickAction) {
    if (v !== "menu" && app.rightClickAction() === "menu") {
      pendingRightClick = v;
      return;
    }
    void app.setRightClickAction(v);
  }

  function confirmRightClick() {
    if (pendingRightClick) void app.setRightClickAction(pendingRightClick);
    pendingRightClick = null;
  }

  /** bind:value already moved the select; resync it to the committed value. */
  function cancelRightClick() {
    rightClickAction = app.rightClickAction();
    pendingRightClick = null;
  }

  async function saveCtrlRightClickMenu() {
    await app.setCtrlRightClickMenu(ctrlRightClickMenu);
  }

  async function saveSftpMaxConcurrent() {
    const clamped = Math.max(sftpBounds.min, Math.min(sftpBounds.max, sftpMaxConcurrent | 0));
    sftpMaxConcurrent = clamped;
    await transfers.setMaxConcurrent(clamped);
  }
</script>

<svelte:window onfocus={loadStartup} />

<div class="page">
  {#if app.capabilities().localPty}
    <div class="section-label" id="local-shell-label">{t("settings.shell.local_shell")}</div>
  {/if}
  <div class="card surface-raised shell-card">
    {#if app.capabilities().localPty}
      <div class="shell-hint">
        {t("settings.shell.pick_hint")}
      </div>
      <div class="radio-group" role="radiogroup" aria-labelledby="local-shell-label">
        {#each shells as sh, i}
          {@const id = `shell-r-${i}`}
          {@const basename = (sh.split("/").pop() || sh).toUpperCase()}
          <div class="radio-wrapper">
            <input type="radio" id={id} name="local-shell" class="radio-state"
                   value={sh} checked={!customMode && (selectedShell === sh || (!selectedShell && shells[0] === sh))}
                   onchange={() => pickShell(sh)} />
            <label for={id} class="radio-label">
              <span class="shell-radio-indicator" aria-hidden="true"></span>
              <span class="info">
                <span class="name">{basename}</span>
                <span class="path">({sh})</span>
              </span>
            </label>
          </div>
        {/each}
        <div class="radio-wrapper">
          <input type="radio" id="shell-r-custom" name="local-shell" class="radio-state"
                 checked={customMode}
                 onchange={pickCustom} />
          <label for="shell-r-custom" class="radio-label">
            <span class="shell-radio-indicator" aria-hidden="true"></span>
            <span class="info">
              <span class="name">{t("settings.shell.custom")}</span>
              <input class="custom-input" type="text"
                     bind:value={customPath}
                     placeholder={t("settings.shell.custom_placeholder")}
                     onfocus={() => pickCustom()}
                     onblur={onCustomBlur} />
            </span>
          </label>
        </div>
      </div>
      <div class="card-divider"></div>
    {/if}
    <div class="cmd-block-head action-row">
      <div class="cmd-block-head-body">
        <label for="startup-action" class="cmd-block-title">{t("settings.shell.startup_action")}</label>
        {#if startupAction === "restore"}
          <div class="cmd-block-desc">{t("settings.shell.startup_restore_desc")}</div>
        {/if}
      </div>
      <div class="action-select">
        <Select id="startup-action" bind:value={startupAction} options={startupOptions}
          disabled={startupBusy || startupAction === null}
          placeholder={startupBusy ? t("common.loading") : ""}
          onchange={(value) => saveStartup(value as StartupAction)} />
      </div>
    </div>
  </div>

  <div class="section-label">{t("settings.shell.connection_settings")}</div>
  <div class="card surface-raised connection-card">
    <div class="cmd-block-head">
      <div class="cmd-block-head-body">
        <label for="connect-timeout" class="cmd-block-title">{t("settings.shell.timeout_label")}</label>
        <div id="connect-timeout-hint" class="cmd-block-desc">{t("settings.shell.timeout_hint")}</div>
      </div>
      <input id="connect-timeout" type="number" bind:value={connectTimeout} min="1" max="300"
        aria-describedby="connect-timeout-hint" onblur={saveTimeout}
        onkeydown={(e) => { if (e.key === "Enter") saveTimeout(); }} />
    </div>

    <div class="card-divider"></div>

    <div class="cmd-block-head">
      <div class="cmd-block-head-body">
        <label for="sftp-concurrent" class="cmd-block-title">{t("settings.shell.sftp_concurrent_label")}</label>
        <div id="sftp-concurrent-hint" class="cmd-block-desc">{t("settings.shell.sftp_concurrent_hint", {
          min: sftpBounds.min, max: sftpBounds.max, def: sftpBounds.def,
        })}</div>
      </div>
      <input id="sftp-concurrent" type="number" bind:value={sftpMaxConcurrent}
        min={sftpBounds.min} max={sftpBounds.max} aria-describedby="sftp-concurrent-hint"
        onblur={saveSftpMaxConcurrent}
        onkeydown={(e) => { if (e.key === "Enter") saveSftpMaxConcurrent(); }} />
    </div>

    <div class="card-divider"></div>

    <div class="cmd-block-head">
      <div class="cmd-block-head-body">
        <label for="ssh-connection-log" class="cmd-block-title" class:on={verboseLog} class:off={!verboseLog}>{t("settings.shell.verbose_log")}</label>
        <div id="ssh-connection-log-hint" class="cmd-block-desc">{t("settings.shell.verbose_log_desc")}</div>
      </div>
      <label class="switch">
        <input id="ssh-connection-log" type="checkbox" bind:checked={verboseLog}
          aria-describedby="ssh-connection-log-hint" onchange={saveVerbose} />
        <span class="slider"></span>
      </label>
    </div>
  </div>

  <div class="section-label">{t("settings.shell.interaction")}</div>
  <!-- 终端交互：选中即复制（开关）+ 关闭标签页确认（开关）+ 右键动作（下拉）合在一张卡片，
       "行 + 分隔线 + 行"结构，跟命令块卡片同款，避免控件割裂。 -->
  <div class="card surface-raised mouse-card">
    <div class="cmd-block-head">
      <div class="cmd-block-head-body">
        <div class="cmd-block-title" class:on={copyOnSelect} class:off={!copyOnSelect}>{t("settings.shell.copy_on_select")}</div>
        <div class="cmd-block-desc">{t("settings.shell.copy_on_select_desc")}</div>
      </div>
      <label class="switch">
        <input type="checkbox" bind:checked={copyOnSelect} onchange={saveCopyOnSelect} />
        <span class="slider"></span>
      </label>
    </div>

    <div class="card-divider"></div>

    <div class="cmd-block-head">
      <div class="cmd-block-head-body">
        <div class="cmd-block-title" class:on={confirmCloseTab} class:off={!confirmCloseTab}>{t("settings.shell.confirm_close_tab")}</div>
        <div class="cmd-block-desc">{t("settings.shell.confirm_close_tab_desc")}</div>
      </div>
      <label class="switch">
        <input type="checkbox" bind:checked={confirmCloseTab} onchange={saveConfirmCloseTab} />
        <span class="slider"></span>
      </label>
    </div>

    <div class="card-divider"></div>

    <div class="cmd-block-head action-row">
      <div class="cmd-block-head-body">
        <label for="rca-select" class="cmd-block-title">{t("settings.shell.right_click")}</label>
        <div class="cmd-block-desc">{t("settings.shell.right_click_desc")}</div>
      </div>
      <div class="action-select">
        <Select id="rca-select" bind:value={rightClickAction}
                options={rightClickOptions}
                onchange={(v) => saveRightClickAction(v as app.RightClickAction)} />
      </div>
    </div>
  </div>

  <!-- Leaving "menu" hides the system right-click menu (entry point for many
       features), so it needs an explicit confirm before committing. -->
  {#if pendingRightClick}
    {@const actionLabel = rightClickOptions.find((o) => o.value === pendingRightClick)?.label ?? ""}
    <Modal onClose={cancelRightClick} class="stack"
           aria-labelledby="rca-confirm-title" aria-describedby="rca-confirm-body">
      <h3 id="rca-confirm-title" class="dialog-title">{t("settings.shell.right_click_confirm_title")}</h3>
      <div id="rca-confirm-body" class="dialog-body">{t("settings.shell.right_click_confirm_body", { action: actionLabel })}</div>
      <div class="dialog-hint">{t("settings.shell.right_click_confirm_hint")}</div>
      <div class="cmd-block-head">
        <div class="cmd-block-head-body">
          <div class="cmd-block-title"
               class:on={ctrlRightClickMenu} class:off={!ctrlRightClickMenu}>
            {t("settings.shell.ctrl_right_click_menu", { mod: modKey })}
          </div>
          <div class="cmd-block-desc">{t("settings.shell.ctrl_right_click_menu_desc", { mod: modKey })}</div>
        </div>
        <label class="switch">
          <input type="checkbox" bind:checked={ctrlRightClickMenu} onchange={saveCtrlRightClickMenu} />
          <span class="slider"></span>
        </label>
      </div>
      <div class="modal-actions">
        <button class="btn btn-sm" onclick={cancelRightClick}>{t("common.cancel")}</button>
        <button class="btn btn-sm btn-primary" onclick={confirmRightClick}>{t("settings.shell.right_click_confirm_ok")}</button>
      </div>
    </Modal>
  {/if}

</div>

<style>
  .page { padding: 24px; display: flex; flex-direction: column; gap: 16px; }

  /* 卡片：复用全局 .card.surface-raised，本地只加 padding + 内布局，
     跟 SyncScreen / AiSettings 同款。 */
  .shell-card,
  .connection-card,
  .mouse-card {
    padding: 18px;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  /* 提示文本：跟 SyncScreen .pat-hint 同一档（11px / text-dim / 行高 1.5）。 */
  .shell-hint {
    font-size: 11px;
    color: var(--text-dim);
    line-height: 1.5;
  }

  /* Radio group —— 复刻 uiverse neu radio：三层圆形阴影（外圈 raised + 内圈 reversed well +
     凸起盖板）。选中时盖板缩小+下移+淡出，露出底下的"井"。
     颜色 token 化：#ecf0f3 → var(--surface)、#d1d9e6 → var(--shadow-dark)、#fff → var(--shadow-light)。
     尺寸：indicator 从参考的 30px 缩到 20px（rssh 字体 13-14px，30 太大），阴影 offset/blur 按比例缩。 */
  .radio-group {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .radio-wrapper {
    position: relative;
  }

  /* 真 input：照搬参考。pointer-events:none → 鼠标穿透到 label，label[for] 转发让 input
     获取 focus；focus 状态触发 `:focus ~ .radio-label .info` 右移 8px。
     注意不能照参考留默认 width/height —— 全局 input 给了 0/0 之外的尺寸会盖住后面 sibling。 */
  .radio-state {
    position: absolute;
    top: 0;
    right: 0;
    width: 1px;
    height: 1px;
    opacity: 1e-5;
    pointer-events: none;
    margin: 0;
    padding: 0;
    box-shadow: none;
  }

  .radio-label {
    display: flex;
    align-items: center;
    gap: 12px;
    cursor: pointer;
    min-height: 20px;
  }

  /* indicator 视觉（.shell-radio-indicator）已提升为全局样式：基座在
     styles/global.css，neu / flat / material 三主题都在 styles/shapes/*.css。 */

  /* 文字：name + path 单行 inline 排列。
     opacity 1 不衰减 —— 之前 0.6→1 的微交互是参考里 `:focus ~` 那套的辅助效果，
     focus 部分删了之后留着反而让 Custom 行的 input placeholder 也跟着淡到 0.36
     几乎看不见。状态对比交给 name 的 accent 配色就够。 */
  .info {
    flex: 1;
    display: inline-flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
  }
  .name {
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
    letter-spacing: 0.04em;
    flex-shrink: 0;
  }
  .radio-state:checked ~ .radio-label .name { color: var(--accent); }
  .path {
    font-size: 11px;
    color: var(--text-dim);
    font-family: var(--term-font);
    word-break: break-all;
  }

  /* Custom radio 行的 input 紧贴 name 后面，跟其它行的 path 同槽位、同样式（dim/monospace）。
     placeholder 写 "(/usr/local/bin/fish)"，括号风格跟内置行的 (/bin/zsh) 一致。 */
  .custom-input {
    flex: 1;
    background: transparent;
    border: none;
    box-shadow: none;
    padding: 0;
    font-size: 11px;
    font-family: var(--term-font);
    color: var(--text-dim);
    border-radius: 0;
    min-width: 0;
  }
  .custom-input:focus { outline: none; color: var(--text); }
  .custom-input::placeholder { color: var(--text-dim); opacity: 0.6; }

  .connection-card input[type="number"] {
    width: 80px;
    flex-shrink: 0;
  }

  /* 动作下拉框：定宽靠右，窄窗口换行并限制在卡片内。 */
  .action-row { flex-wrap: wrap; }
  .action-row .cmd-block-head-body { flex-basis: 180px; }
  .action-select { width: 260px; max-width: 100%; flex-shrink: 0; margin-left: auto; }

  /* 交互卡片的行布局（title/desc + 控件），分隔线分隔多行。
     类名沿用 .cmd-block-* —— 命令块卡片已迁到 CommandBlockSettings，这里复用同款行结构。 */
  .cmd-block-head {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .cmd-block-head-body {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .cmd-block-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--text);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .cmd-block-title.on { color: var(--accent); }
  .cmd-block-desc {
    font-size: 11px;
    color: var(--text-dim);
    line-height: 1.5;
  }

  /* 卡片内分隔线：负边距贯穿到卡片左右边缘。 */
  .card-divider {
    height: 1px;
    background: var(--divider);
    margin: 2px -18px;
  }

  /* Right-click confirm dialog — shell (scrim + card) comes from Modal.svelte,
     title/body typography from global .dialog-title/.dialog-body. */
  .dialog-hint {
    font-size: 11px;
    color: var(--text-dim);
    line-height: 1.5;
  }
</style>
