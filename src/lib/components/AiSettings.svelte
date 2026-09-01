<script lang="ts">
    import { onMount, onDestroy } from "svelte";
    import { invoke } from "@tauri-apps/api/core";
    import * as ai from "../ai/store.svelte.ts";
    import { t, errMsg } from "../i18n/index.svelte.ts";
    import type { AiProviderRecord, CategoryGroup, LlmProtocol, RedactRuleRecord, SkillRecord } from "../ai/types.ts";
    import AiProviderForm from "./AiProviderForm.svelte";
    import DangerModeToggle from "../ai/DangerModeToggle.svelte";
    import AppIcon from "./AppIcon.svelte";

    function openExternal(e: MouseEvent, url: string) {
        e.preventDefault();
        invoke("open_external_url", { url }).catch(err =>
            console.error("open_external_url failed:", err)
        );
    }

    // ─── Provider 管理（动态发现式：列表 + 内联表单）────────────
    // 数据模型：ai_providers 表一行一个 provider（name/protocol/endpoint/model
    // + key 在 secret store）。协议三选一；endpoint 必填，chips 一键填官方值。
    let providers = $state<AiProviderRecord[]>([]);
    /** 当前 active 行 id（`ai_provider` settings 键的镜像）。 */
    let activeId = $state("");
    let adding = $state<AiProviderRecord | null>(null);
    let addKey = $state(0);
    let editId = $state<string | null>(null);
    let byokNote = $state<string | null>(null);
    /** byokNote 自清 timer 句柄，避免后续动作被旧 timer 误清。 */
    let byokNoteTimer: number | null = null;
    // 二次点击删除确认（独立 timer，跟 skill/规则的管理段同款）。
    let confirmingDeleteId = $state<string | null>(null);
    let providerDeleteTimer: number | null = null;
    // 激活串行化：同一时刻最多一个 activate 在途，radio 随之禁用 ——
    // 慢失败的老请求不可能把过期的 previousId 盖到新选择上。
    let activating = $state(false);

    /** 协议三卡 —— 表单顶部的类型选择（对应动态发现的 Docker/kubectl 卡）。
     * 副行文案走 i18n，卡标题是专有名词不翻译。 */
    const PROTOCOL_CARDS: { protocol: LlmProtocol; label: string; subKey: string }[] = [
        { protocol: "deepseek-thinking", label: "DeepSeek Thinking", subKey: "ai.settings.protocol.sub.deepseek" },
        { protocol: "openai-completions", label: "OpenAI Completions", subKey: "ai.settings.protocol.sub.openai" },
        { protocol: "anthropic-messages", label: "Anthropic Messages", subKey: "ai.settings.protocol.sub.anthropic" },
    ];

    /** endpoint 快捷填充 —— 对应凭证页的 ~/.ssh/id_rsa / id_ed25519 chips。 */
    const ENDPOINT_CHIPS: Record<LlmProtocol, { label: string; url: string }[]> = {
        "deepseek-thinking": [{ label: "DeepSeek", url: "https://api.deepseek.com/v1" }],
        "openai-completions": [
            { label: "OpenAI", url: "https://api.openai.com/v1" },
            { label: "GLM", url: "https://open.bigmodel.cn/api/paas/v4" },
        ],
        "anthropic-messages": [
            { label: "Anthropic", url: "https://api.anthropic.com/v1/messages" },
        ],
    };

    function protocolLabel(protocol: string): string {
        return PROTOCOL_CARDS.find((c) => c.protocol === protocol)?.label ?? protocol;
    }

    // ─── Danger mode（全局，跟 provider 无关）────────────────────────
    // dangerMode 直接派生自 store —— DangerModeToggle 改动后这里自动同步，不再维护
    // 本地镜像。saving 状态与确认模态都归 DangerModeToggle 管。
    let dangerMode = $derived(ai.settings()?.danger_mode === true);
    let dangerNote = $state<string | null>(null);

    // per-tool 自动批准。每个 checkbox 各自一个 boolean state。8 个字段平铺；
    // 也可以塞成数组遍历，但显式 boolean 比 metadata 表调试时更直观，也避免一行
    // toggle bug 同时拆掉 8 个开关。
    let autoRunCommand = $state(true);
    let autoMatchFile = $state(true);
    let autoDownloadFile = $state(false);
    let autoAnalyzeLocally = $state(false);
    let autoPatchCp = $state(false);
    let autoPatchModify = $state(false);
    let autoPatchDiff = $state(false);
    let autoPatchMv = $state(false);
    let autoWebSearch = $state(false);
    let autoWebFetch = $state(false);
    let savingAuto = $state(false);

    // ─── 远端 shell 自动探测（与 danger_mode 解耦的独立开关）─────────
    // 默认 off：99% Linux/macOS 远端假设 POSIX 即正确，零探测开销保持现状。
    // 用户连 Windows / 改了 DefaultShell 的远端时手动开启，每次 SSH 连接成功后跑
    // 一行 echo 探针自动定位 cmd.exe / PowerShell（对已连接会话开启需重连生效）。
    let autoDetectRemoteShell = $state(false);
    let savingShellDetect = $state(false);
    let shellDetectNote = $state<string | null>(null);

    async function persistAutoDetectShell(next: boolean) {
        savingShellDetect = true;
        shellDetectNote = null;
        try {
            await ai.saveSettings({ autoDetectRemoteShell: next });
        } catch (err) {
            // 失败回滚 UI 状态，保持跟后端单一真相
            autoDetectRemoteShell = !next;
            shellDetectNote = t("ai.settings.shell_detect.save_failed", { error: errMsg(err) });
        } finally {
            savingShellDetect = false;
        }
    }

    /** 子开关写回后端。失败把 UI 状态回滚到 prev，避免界面与持久化失同步。 */
    async function persistAuto(field: "autoRunCommand" | "autoMatchFile" | "autoDownloadFile"
                                    | "autoAnalyzeLocally" | "autoPatchCp" | "autoPatchModify"
                                    | "autoPatchDiff" | "autoPatchMv" | "autoWebSearch" | "autoWebFetch",
                               next: boolean) {
        savingAuto = true;
        dangerNote = null;
        try {
            await ai.saveSettings({ [field]: next });
        } catch (err) {
            // Enabling failed → restore off. Disabling failed stays off locally:
            // safety revocation is immediate and must not be undone by a DB error.
            if (next) {
                switch (field) {
                    case "autoRunCommand":     autoRunCommand     = false; break;
                    case "autoMatchFile":      autoMatchFile      = false; break;
                    case "autoDownloadFile":   autoDownloadFile   = false; break;
                    case "autoAnalyzeLocally": autoAnalyzeLocally = false; break;
                    case "autoPatchCp":        autoPatchCp        = false; break;
                    case "autoPatchModify":    autoPatchModify    = false; break;
                    case "autoPatchDiff":      autoPatchDiff      = false; break;
                    case "autoPatchMv":        autoPatchMv        = false; break;
                    case "autoWebSearch":      autoWebSearch      = false; break;
                    case "autoWebFetch":       autoWebFetch       = false; break;
                }
            }
            dangerNote = t("ai.settings.danger.save_failed", { error: errMsg(err) });
        } finally {
            savingAuto = false;
        }
    }
    function setByokNote(msg: string | null, autoClearMs?: number) {
        if (byokNoteTimer !== null) {
            clearTimeout(byokNoteTimer);
            byokNoteTimer = null;
        }
        byokNote = msg;
        if (msg !== null && autoClearMs !== undefined) {
            byokNoteTimer = window.setTimeout(() => {
                byokNote = null;
                byokNoteTimer = null;
            }, autoClearMs);
        }
    }

    async function refreshProviders() {
        try {
            providers = await ai.loadProviders();
        } catch (e: any) {
            setByokNote(t("ai.settings.note.providers_failed", { error: errMsg(e) }));
        }
    }

    /** 新建：空表单（默认协议 = 第一张卡）。 */
    function startAdd() {
        editId = null;
        addKey += 1;
        adding = blankProvider();
    }

    function startEdit(p: AiProviderRecord) {
        adding = null;
        editId = p.id;
    }

    function cancelForm() {
        adding = null;
        editId = null;
    }

    function blankProvider(): AiProviderRecord {
        return {
            id: "",
            name: "",
            protocol: "openai-completions",
            model: "",
            endpoint: "",
            has_api_key: false,
        };
    }

    /** Activate a provider row (the list's radio). The check follows activeId
     * optimistically — Svelte re-applies both rows on every change, so a
     * failure restores the previous selection (a native radio group never
     * re-checks the old member by itself). With one activation in flight at
     * a time, that restore can never clobber a newer selection. */
    async function activate(id: string) {
        const previousId = activeId;
        if (id === previousId || activating) return;
        activating = true;
        activeId = id;
        try {
            await ai.activateProvider(id);
        } catch (e: any) {
            activeId = previousId;
            setByokNote(t("ai.settings.note.save_failed", { error: errMsg(e) }));
        } finally {
            activating = false;
        }
    }

    async function removeProvider(p: AiProviderRecord) {
        // 二次点击确认：3s 内不再点就回退。
        if (confirmingDeleteId !== p.id) {
            confirmingDeleteId = p.id;
            if (providerDeleteTimer !== null) clearTimeout(providerDeleteTimer);
            providerDeleteTimer = window.setTimeout(() => {
                confirmingDeleteId = null;
                providerDeleteTimer = null;
            }, 3000);
            return;
        }
        confirmingDeleteId = null;
        if (providerDeleteTimer !== null) {
            clearTimeout(providerDeleteTimer);
            providerDeleteTimer = null;
        }
        try {
            const wasActive = activeId === p.id;
            await ai.deleteProvider(p.id);
            if (editId === p.id) editId = null;
            if (wasActive) {
                // 后端已清 active；刷新全局 settings 快照，否则 ChatPanel 还
                // 拿着已删 id 去 start（provider_not_found）。
                await ai.loadSettings();
                activeId = "";
            }
            await refreshProviders();
        } catch (e: any) {
            setByokNote(t("ai.settings.note.delete_failed", { error: errMsg(e) }));
        }
    }

    /** 表单保存回调（新建/编辑共用；表单组件内部做校验）。 */
    async function onProviderSaved(id: string) {
        cancelForm();
        await refreshProviders();
        // 首个 provider 落地即激活 —— 消灭"建了 provider 但没有 active"的死角。
        // 激活失败要报出来且不认领 activeId，否则 UI 显示"使用中"是假的。
        if (!activeId) {
            try {
                await ai.activateProvider(id);
                activeId = id;
            } catch (e: any) {
                setByokNote(t("ai.settings.note.save_failed", { error: errMsg(e) }));
                return;
            }
        }
        setByokNote(t("ai.settings.note.saved"), 2000);
    }

    // ─── Skill 管理 ────────────────────────────────────────────
    let skills = $state<SkillRecord[]>([]);
    let editing = $state<SkillRecord | null>(null);
    let isNew = $state(false);
    let savingSkill = $state(false);
    let skillNote = $state<string | null>(null);
    let confirmingDelete = $state(false);
    let confirmDeleteTimer: number | null = null;

    function resetDeleteConfirm() {
        confirmingDelete = false;
        if (confirmDeleteTimer !== null) {
            clearTimeout(confirmDeleteTimer);
            confirmDeleteTimer = null;
        }
    }

    // ─── 脱敏规则管理 ──────────────────────────────────────────
    // 镜像 Skill 管理：列表 + 行内编辑表单 + 二次点击删除确认。规则没有 builtin
    // 概念（默认已 seed 进 DB），全部可改可删。变更只对新会话生效。
    let redactRules = $state<RedactRuleRecord[]>([]);
    let editingRule = $state<RedactRuleRecord | null>(null);
    let isNewRule = $state(false);
    let savingRule = $state(false);
    let ruleNote = $state<string | null>(null);
    // 独立于 skill 的删除确认状态，避免两处共用一个 flag 互相串台。
    let confirmingRuleDelete = $state(false);
    let confirmRuleDeleteTimer: number | null = null;

    function resetRuleDeleteConfirm() {
        confirmingRuleDelete = false;
        if (confirmRuleDeleteTimer !== null) {
            clearTimeout(confirmRuleDeleteTimer);
            confirmRuleDeleteTimer = null;
        }
    }

    onMount(async () => {
        const s = await ai.loadSettings();
        activeId = s.provider;
        autoRunCommand = s.auto_run_command;
        autoMatchFile = s.auto_match_file;
        autoDownloadFile = s.auto_download_file;
        autoAnalyzeLocally = s.auto_analyze_locally;
        autoPatchCp = s.auto_patch_cp;
        autoPatchModify = s.auto_patch_modify;
        autoPatchDiff = s.auto_patch_diff;
        autoPatchMv = s.auto_patch_mv;
        autoWebSearch = s.auto_web_search;
        autoWebFetch = s.auto_web_fetch;
        autoDetectRemoteShell = s.auto_detect_remote_shell;
        await refreshProviders();
        await refreshSkills();
        await refreshRedactRules();
        await refreshBlacklist();
    });

    onDestroy(() => {
        if (byokNoteTimer !== null) clearTimeout(byokNoteTimer);
        if (providerDeleteTimer !== null) clearTimeout(providerDeleteTimer);
        if (confirmDeleteTimer !== null) clearTimeout(confirmDeleteTimer);
        if (confirmRuleDeleteTimer !== null) clearTimeout(confirmRuleDeleteTimer);
    });

    async function refreshSkills() {
        try {
            skills = await ai.listSkills();
        } catch (e) {
            skillNote = t("ai.settings.skills.error.load_failed", { error: errMsg(e) });
        }
    }

    function newSkill() {
        editing = {
            id: "user-" + crypto.randomUUID().slice(0, 8),
            name: "",
            description: "",
            content: "",
            builtin: false,
        };
        isNew = true;
        skillNote = null;
        resetDeleteConfirm();
    }

    function viewSkill(s: SkillRecord) {
        editing = { ...s };
        isNew = false;
        skillNote = null;
        resetDeleteConfirm();
    }

    function cancelEdit() {
        editing = null;
        isNew = false;
        skillNote = null;
        resetDeleteConfirm();
    }

    async function saveSkill() {
        if (!editing) return;
        if (editing.builtin) {
            skillNote = t("ai.settings.skills.error.builtin_readonly");
            return;
        }
        if (!editing.name.trim() || !editing.content.trim() || !editing.id.trim()) {
            skillNote = t("ai.settings.skills.error.empty_fields");
            return;
        }
        savingSkill = true;
        skillNote = null;
        try {
            await ai.saveSkill({
                id: editing.id.trim(),
                name: editing.name.trim(),
                description: editing.description.trim(),
                content: editing.content,
            });
            editing = null;
            isNew = false;
            await refreshSkills();
        } catch (e) {
            skillNote = t("ai.settings.skills.error.save_failed", { error: errMsg(e) });
        } finally {
            savingSkill = false;
        }
    }

    async function removeSkill(s: SkillRecord) {
        if (s.builtin) return;
        // 二次点击确认：第一次切到 "click again to confirm" 状态，3s 内不再点就回退
        if (!confirmingDelete) {
            confirmingDelete = true;
            confirmDeleteTimer = window.setTimeout(() => {
                confirmingDelete = false;
                confirmDeleteTimer = null;
            }, 3000);
            return;
        }
        resetDeleteConfirm();
        try {
            await ai.deleteSkill(s.id);
            editing = null;
            isNew = false;
            await refreshSkills();
        } catch (e) {
            skillNote = t("ai.settings.skills.error.delete_failed", { error: errMsg(e) });
        }
    }

    async function refreshRedactRules() {
        try {
            redactRules = await ai.listRedactRules();
        } catch (e) {
            ruleNote = t("ai.settings.redact.error.load_failed", { error: errMsg(e) });
        }
    }

    function newRule() {
        editingRule = {
            id: "user-" + crypto.randomUUID().slice(0, 8),
            pattern: "",
            replacement: "",
        };
        isNewRule = true;
        ruleNote = null;
        resetRuleDeleteConfirm();
    }

    function viewRule(r: RedactRuleRecord) {
        editingRule = { ...r };
        isNewRule = false;
        ruleNote = null;
        resetRuleDeleteConfirm();
    }

    function cancelRuleEdit() {
        editingRule = null;
        isNewRule = false;
        ruleNote = null;
        resetRuleDeleteConfirm();
    }

    async function saveRule() {
        if (!editingRule) return;
        // 空串校验用 === ""（不 trim）：正则里前后空格可能有意义；空 pattern 会匹配
        // 每个位置导致灾难性替换，必须挡掉。后端还会再编译校验一次。
        if (editingRule.pattern === "" || editingRule.replacement === "") {
            ruleNote = t("ai.settings.redact.error.empty_fields");
            return;
        }
        savingRule = true;
        ruleNote = null;
        try {
            await ai.saveRedactRule({
                id: editingRule.id,
                pattern: editingRule.pattern,
                replacement: editingRule.replacement,
            });
            editingRule = null;
            isNewRule = false;
            await refreshRedactRules();
        } catch (e) {
            // 坏正则在后端被拒，errMsg 解析出 error.redact_invalid_regex 的中文/英文消息。
            ruleNote = t("ai.settings.redact.error.save_failed", { error: errMsg(e) });
        } finally {
            savingRule = false;
        }
    }

    async function removeRule(r: RedactRuleRecord) {
        if (!confirmingRuleDelete) {
            confirmingRuleDelete = true;
            confirmRuleDeleteTimer = window.setTimeout(() => {
                confirmingRuleDelete = false;
                confirmRuleDeleteTimer = null;
            }, 3000);
            return;
        }
        resetRuleDeleteConfirm();
        try {
            await ai.deleteRedactRule(r.id);
            editingRule = null;
            isNewRule = false;
            await refreshRedactRules();
        } catch (e) {
            ruleNote = t("ai.settings.redact.error.delete_failed", { error: errMsg(e) });
        }
    }

    // ─── 命令黑名单管理 ──────────────────────────────────────────
    // 五类，每类一行，整类编辑：点行 → textarea 改 → 保存即整类替换。
    // 没有 builtin 概念（默认已 seed 进 DB），全部可改；空一类 = 放行该类。
    // 变更只对新会话生效。
    let blacklist = $state<CategoryGroup[]>([]);
    let editingCat = $state<string | null>(null);
    let editingCmds = $state("");
    let savingCat = $state(false);
    let blNote = $state<string | null>(null);

    // 分类 key → i18n 标签。t() 的 key 是字面量联合类型，不能动态拼，所以这里 switch。
    function catLabel(cat: string): string {
        switch (cat) {
            case "destructive": return t("ai.settings.blacklist.cat.destructive");
            case "write_verb": return t("ai.settings.blacklist.cat.write_verb");
            case "interpreter": return t("ai.settings.blacklist.cat.interpreter");
            case "deferred_exec": return t("ai.settings.blacklist.cat.deferred_exec");
            case "forwarder": return t("ai.settings.blacklist.cat.forwarder");
            default: return cat;
        }
    }

    async function refreshBlacklist() {
        try {
            blacklist = await ai.listCommandBlacklist();
        } catch (e) {
            blNote = t("ai.settings.blacklist.error.load_failed", { error: errMsg(e) });
        }
    }

    function editCat(g: CategoryGroup) {
        editingCat = g.category;
        editingCmds = g.commands.join(" ");
        blNote = null;
    }

    function cancelCatEdit() {
        editingCat = null;
        blNote = null;
    }

    async function saveCat() {
        if (!editingCat) return;
        // 空格 / 逗号 / 换行分隔，去空。后端还会再校验每个命令名。
        const names = editingCmds.split(/[\s,]+/).filter(Boolean);
        savingCat = true;
        blNote = null;
        try {
            await ai.replaceCommandBlacklist(editingCat, names);
            editingCat = null;
            await refreshBlacklist();
        } catch (e) {
            blNote = t("ai.settings.blacklist.error.save_failed", { error: errMsg(e) });
        } finally {
            savingCat = false;
        }
    }
</script>

<div class="page">
    <div class="section-label">{t("ai.settings.section.provider")}</div>
    <!-- Provider 管理 + BYOK 警告合在一个 .card.surface-raised。
         交互照动态发现：顶部"新建"，列表行（active 单选 / 编辑 / 删除二次确认），
         新建或编辑时渲染内联表单 AiProviderForm（三协议卡 + endpoint chips）。 -->
    <div class="card surface-raised provider-card">
        <div class="warn">
            <AppIcon name="warning" size={16} />
            <span>
                {t("ai.settings.warn.byok")}
                （<a href="https://www.anthropic.com/legal/privacy" onclick={(e) => openExternal(e, "https://www.anthropic.com/legal/privacy")}>Anthropic</a>
                 / <a href="https://openai.com/policies/privacy-policy/" onclick={(e) => openExternal(e, "https://openai.com/policies/privacy-policy/")}>OpenAI</a>
                 / <a href="https://platform.deepseek.com/downloads" onclick={(e) => openExternal(e, "https://platform.deepseek.com/downloads")}>DeepSeek</a>
                 / <a href="https://docs.bigmodel.cn/cn/terms/privacy-policy" onclick={(e) => openExternal(e, "https://docs.bigmodel.cn/cn/terms/privacy-policy")}>GLM</a>）。
            </span>
        </div>

        <div class="card-head">
            <span class="hint">{t("ai.settings.provider.hint")}</span>
            {#if !adding}
                <button class="btn btn-sm" onclick={startAdd}>{t("ai.settings.provider.new")}</button>
            {/if}
        </div>

        {#if adding}
            {#key addKey}
                <AiProviderForm
                    provider={adding}
                    protocolCards={PROTOCOL_CARDS}
                    endpointChips={ENDPOINT_CHIPS}
                    onSave={onProviderSaved}
                    onCancel={cancelForm}
                />
            {/key}
        {/if}

        {#each providers as p (p.id)}
            {#if editId === p.id}
                <AiProviderForm
                    provider={p}
                    protocolCards={PROTOCOL_CARDS}
                    endpointChips={ENDPOINT_CHIPS}
                    onSave={onProviderSaved}
                    onCancel={cancelForm}
                />
            {:else}
                <div class="provider-row">
                    <div class="provider-info">
                        <input type="radio" id={`ai-provider-r-${p.id}`} name="ai-provider" class="radio-state"
                               checked={activeId === p.id}
                               onchange={() => activate(p.id)}
                               disabled={activating} />
                        <label for={`ai-provider-r-${p.id}`} class="radio-label" title={t("ai.settings.provider.activate")}>
                            <span class="shell-radio-indicator" aria-hidden="true"></span>
                            <div class="provider-text">
                                <div class="provider-name" title={p.name}>{p.name}</div>
                                <div class="provider-sub" title={`${protocolLabel(p.protocol)} · ${p.endpoint} · ${p.model}`}>
                                    {protocolLabel(p.protocol)} · {p.endpoint} · {p.model}
                                </div>
                            </div>
                        </label>
                    </div>
                    <div class="provider-actions">
                        {#if activeId === p.id}
                            <span class="active-badge">{t("ai.settings.provider.active")}</span>
                        {/if}
                        <button
                            class="btn btn-sm btn-icon"
                            title={t("common.edit")}
                            aria-label={`${t("common.edit")} ${p.name}`}
                            onclick={() => startEdit(p)}
                        >
                            <AppIcon name="edit" size={16} />
                        </button>
                        <!-- Two-tap delete: trash icon first, then the button morphs
                             into an explicit text confirm (3s timeout reverts). -->
                        {#if confirmingDeleteId === p.id}
                            <button class="btn btn-sm btn-danger" onclick={() => removeProvider(p)}>
                                {t("ai.settings.provider.delete_confirm")}
                            </button>
                        {:else}
                            <button
                                class="btn btn-sm btn-icon btn-danger"
                                title={t("common.delete")}
                                aria-label={`${t("common.delete")} ${p.name}`}
                                onclick={() => removeProvider(p)}
                            >
                                <AppIcon name="trash" size={16} />
                            </button>
                        {/if}
                    </div>
                </div>
            {/if}
        {:else}
            {#if !adding}
                <div class="placeholder">{t("ai.settings.provider.empty")}</div>
            {/if}
        {/each}

        {#if byokNote}<span class="note">{byokNote}</span>{/if}
    </div>

    <div class="section-label">{t("ai.settings.danger.section")}</div>
    <!-- 危险模式 + 8 个 per-tool 自动批准合在一个 .card.surface-raised（参考 SyncScreen）。
         视觉上是一组语义关联的配置，不再拆成两个浮空卡片。 -->
    <div class="card surface-raised danger-card" class:on={dangerMode}>
        <div class="danger-head">
            <div class="danger-head-body">
                <div id="danger-mode-title" class="danger-title"
                     class:on={dangerMode} class:off={!dangerMode}>
                    {t("ai.settings.danger.label")}
                </div>
                <div id="danger-mode-desc" class="danger-desc">{t("ai.settings.danger.desc")}</div>
                {#if dangerNote}
                    <div class="danger-err">{dangerNote}</div>
                {/if}
            </div>
            <DangerModeToggle onError={(m) => (dangerNote = t("ai.settings.danger.save_failed", { error: m }))}>
                {#snippet trigger(requestToggle, saving)}
                    <label class="switch">
                        <input type="checkbox" checked={dangerMode}
                               disabled={saving}
                               onclick={(e) => { e.preventDefault(); dangerNote = null; requestToggle(); }}
                               aria-labelledby="danger-mode-title"
                               aria-describedby="danger-mode-desc"/>
                        <span class="slider"></span>
                    </label>
                {/snippet}
            </DangerModeToggle>
        </div>

        <div class="card-divider"></div>

        <!-- per-tool 自动批准。danger_mode 关时整组 disabled —— 视觉灰显，不隐藏，
             让用户知道这些选项存在、是怎么分粒度的；开 danger 时立刻可用。 -->
        <div class="auto-group" class:disabled={!dangerMode}>
            <div class="auto-group-title">{t("ai.settings.danger.auto.section")}</div>
            <label class="auto-row">
                <input type="checkbox" bind:checked={autoRunCommand}
                       disabled={!dangerMode || savingAuto}
                       onchange={(e) => persistAuto("autoRunCommand", (e.target as HTMLInputElement).checked)}/>
                <span>{t("ai.settings.danger.auto.run_command")}</span>
            </label>
            <label class="auto-row">
                <input type="checkbox" bind:checked={autoMatchFile}
                       disabled={!dangerMode || savingAuto}
                       onchange={(e) => persistAuto("autoMatchFile", (e.target as HTMLInputElement).checked)}/>
                <span>{t("ai.settings.danger.auto.match_file")}</span>
            </label>
            <label class="auto-row">
                <input type="checkbox" bind:checked={autoDownloadFile}
                       disabled={!dangerMode || savingAuto}
                       onchange={(e) => persistAuto("autoDownloadFile", (e.target as HTMLInputElement).checked)}/>
                <span>{t("ai.settings.danger.auto.download_file")}</span>
            </label>
            <label class="auto-row">
                <input type="checkbox" bind:checked={autoAnalyzeLocally}
                       disabled={!dangerMode || savingAuto}
                       onchange={(e) => persistAuto("autoAnalyzeLocally", (e.target as HTMLInputElement).checked)}/>
                <span>{t("ai.settings.danger.auto.analyze_locally")}</span>
            </label>
            <label class="auto-row">
                <input type="checkbox" bind:checked={autoPatchCp}
                       disabled={!dangerMode || savingAuto}
                       onchange={(e) => persistAuto("autoPatchCp", (e.target as HTMLInputElement).checked)}/>
                <span>{t("ai.settings.danger.auto.patch_cp")}</span>
            </label>
            <label class="auto-row">
                <input type="checkbox" bind:checked={autoPatchModify}
                       disabled={!dangerMode || savingAuto}
                       onchange={(e) => persistAuto("autoPatchModify", (e.target as HTMLInputElement).checked)}/>
                <span>{t("ai.settings.danger.auto.patch_modify")}</span>
            </label>
            <label class="auto-row">
                <input type="checkbox" bind:checked={autoPatchDiff}
                       disabled={!dangerMode || savingAuto}
                       onchange={(e) => persistAuto("autoPatchDiff", (e.target as HTMLInputElement).checked)}/>
                <span>{t("ai.settings.danger.auto.patch_diff")}</span>
            </label>
            <label class="auto-row">
                <input type="checkbox" bind:checked={autoPatchMv}
                       disabled={!dangerMode || savingAuto}
                       onchange={(e) => persistAuto("autoPatchMv", (e.target as HTMLInputElement).checked)}/>
                <span>{t("ai.settings.danger.auto.patch_mv")}</span>
            </label>
            <label class="auto-row">
                <input type="checkbox" bind:checked={autoWebSearch}
                       disabled={!dangerMode || savingAuto}
                       onchange={(e) => persistAuto("autoWebSearch", (e.target as HTMLInputElement).checked)}/>
                <span>{t("ai.settings.danger.auto.web_search")}</span>
            </label>
            <label class="auto-row">
                <input type="checkbox" bind:checked={autoWebFetch}
                       disabled={!dangerMode || savingAuto}
                       onchange={(e) => persistAuto("autoWebFetch", (e.target as HTMLInputElement).checked)}/>
                <span>{t("ai.settings.danger.auto.web_fetch")}</span>
            </label>
        </div>
    </div>

    <!-- 远端 shell 自动探测 —— 独立卡片，跟 danger_mode 解耦。
         off（默认）：远端假设 POSIX，保持 99% 用户零开销。
         on：SSH 连接成功后跑一行 echo 探针，定位 cmd.exe / PowerShell。 -->
    <div class="card surface-raised">
        <div class="danger-head">
            <div class="danger-head-body">
                <div id="shell-detect-title" class="danger-title">
                    {t("ai.settings.shell_detect.label")}
                </div>
                <div id="shell-detect-desc" class="danger-desc">{t("ai.settings.shell_detect.desc")}</div>
                {#if shellDetectNote}
                    <div class="danger-err">{shellDetectNote}</div>
                {/if}
            </div>
            <label class="switch">
                <input type="checkbox" bind:checked={autoDetectRemoteShell}
                       disabled={savingShellDetect}
                       onchange={(e) => persistAutoDetectShell((e.target as HTMLInputElement).checked)}
                       aria-labelledby="shell-detect-title"
                       aria-describedby="shell-detect-desc"/>
                <span class="slider"></span>
            </label>
        </div>
    </div>

    <!-- 脱敏规则管理 —— 镜像下方 Skill 段：列表 / 行内表单 / 二次删除确认。 -->
    <div class="section-label">{t("ai.settings.section.redact")}</div>
    <!-- 脱敏规则整块装进 .card.surface-raised（同 provider-card）；新建按钮移进卡片内的 card-head。 -->
    <div class="card surface-raised list-card">
        <div class="card-head">
            <span class="hint">{t("ai.settings.redact.hint")}</span>
            {#if !editingRule}
                <button class="btn btn-sm" onclick={newRule}>{t("ai.settings.redact.new")}</button>
            {/if}
        </div>

        {#if ruleNote}
            <div class="banner">{ruleNote} <button class="banner-close" onclick={() => (ruleNote = null)} aria-label={t("common.close")}>×</button></div>
        {/if}

        {#if !editingRule}
            <div class="skill-list">
                {#each redactRules as r (r.id)}
                    <button class="skill-item surface-raised-sm" onclick={() => viewRule(r)}>
                        <div class="rule-line">
                            <code class="rule-pattern">{r.pattern}</code>
                            <span class="rule-arrow">→</span>
                            <code class="rule-replacement">{r.replacement}</code>
                        </div>
                    </button>
                {/each}
                {#if redactRules.length === 0}
                    <div class="placeholder">{t("ai.settings.redact.empty")}</div>
                {/if}
            </div>
        {:else}
            <div class="form">
                <div class="row">
                    <label for="rr-pattern">{t("ai.settings.redact.label.pattern")}</label>
                    <input id="rr-pattern" type="text" class="mono" bind:value={editingRule.pattern}
                           placeholder={t("ai.settings.redact.placeholder.pattern")}/>
                </div>
                <div class="row">
                    <label for="rr-replacement">{t("ai.settings.redact.label.replacement")}</label>
                    <input id="rr-replacement" type="text" class="mono" bind:value={editingRule.replacement}
                           placeholder={t("ai.settings.redact.placeholder.replacement")}/>
                </div>
                <div class="actions">
                    <button class="btn btn-accent btn-sm" onclick={saveRule} disabled={savingRule}>
                        {savingRule ? t("ai.settings.btn.saving") : t("common.save")}
                    </button>
                    {#if !isNewRule}
                        <button class="btn btn-sm btn-danger" class:confirming={confirmingRuleDelete}
                                onclick={() => editingRule && removeRule(editingRule)}>
                            {confirmingRuleDelete ? t("ai.settings.redact.btn.delete_confirm") : t("ai.settings.redact.btn.delete")}
                        </button>
                    {/if}
                    <button class="btn btn-sm" onclick={cancelRuleEdit}>{t("ai.settings.redact.btn.cancel")}</button>
                </div>
            </div>
        {/if}
    </div>

    <!-- 命令黑名单 + 可用性过滤 合进一个 .card.surface-raised。
         黑名单：五类整类编辑；可用性：只读，照 ShellSettings 的 tips-list 样式。 -->
    <div class="section-label">{t("ai.settings.section.blacklist")}</div>
    <div class="card surface-raised list-card">
        <div class="hint">{t("ai.settings.blacklist.hint")}</div>

        {#if blNote}
            <div class="banner">{blNote} <button class="banner-close" onclick={() => (blNote = null)} aria-label={t("common.close")}>×</button></div>
        {/if}

        <div class="skill-list">
            {#each blacklist as g (g.category)}
                {#if editingCat === g.category}
                    <div class="form">
                        <div class="row">
                            <label for="bl-cmds">{catLabel(g.category)}</label>
                            <textarea id="bl-cmds" class="mono bl-textarea" bind:value={editingCmds}
                                      placeholder={t("ai.settings.blacklist.placeholder")}></textarea>
                        </div>
                        <div class="actions">
                            <button class="btn btn-accent btn-sm" onclick={saveCat} disabled={savingCat}>
                                {savingCat ? t("ai.settings.btn.saving") : t("common.save")}
                            </button>
                            <button class="btn btn-sm" onclick={cancelCatEdit}>{t("ai.settings.redact.btn.cancel")}</button>
                        </div>
                    </div>
                {:else}
                    <button class="skill-item surface-raised-sm" onclick={() => editCat(g)}>
                        <div class="bl-row">
                            <span class="bl-cat">{catLabel(g.category)}</span>
                            <code class="bl-cmds" class:bl-empty={g.commands.length === 0}>
                                {g.commands.length ? g.commands.join("  ") : t("ai.settings.blacklist.empty_cat")}
                            </code>
                        </div>
                    </button>
                {/if}
            {/each}
        </div>

        <div class="card-divider"></div>

        <!-- 可用性过滤 —— 只读说明，非安全拦截。tips-group / tips-list 同 ShellSettings。 -->
        <div class="tips-group">
            <div class="tips-title">{t("ai.settings.section.usability")}</div>
            <div class="hint">{t("ai.settings.usability.intro")}</div>
            <ul class="tips-list">
                <li>{t("ai.settings.usability.item_tui")}</li>
                <li>{t("ai.settings.usability.item_loop")}</li>
            </ul>
        </div>
    </div>

    <div class="section-label skill-header">
        {t("ai.settings.section.skills")}
        {#if !editing}
            <button class="btn btn-sm" onclick={newSkill}>{t("ai.settings.skills.new")}</button>
        {/if}
    </div>

    {#if skillNote}
        <div class="banner">{skillNote} <button class="banner-close" onclick={() => (skillNote = null)} aria-label={t("common.close")}>×</button></div>
    {/if}

    {#if !editing}
        <div class="skill-list">
            {#each skills as s (s.id)}
                <button class="skill-item neu-sm" onclick={() => viewSkill(s)}>
                    <div class="skill-row">
                        <span class="skill-name">{s.name}</span>
                        <span class="skill-tag" class:builtin={s.builtin} class:user={!s.builtin}>
                            {s.builtin ? t("ai.settings.skills.tag.builtin") : t("ai.settings.skills.tag.user")}
                        </span>
                        <span class="skill-id">{s.id}</span>
                    </div>
                    {#if s.description}<div class="skill-desc">{s.description}</div>{/if}
                </button>
            {/each}
            {#if skills.length === 0}
                <div class="placeholder">{t("ai.settings.skills.empty")}</div>
            {/if}
        </div>
    {:else}
        <div class="form">
            <div class="row">
                <label for="sk-id">{t("ai.settings.skills.label.id")}</label>
                <input id="sk-id" type="text" bind:value={editing.id}
                       disabled={!isNew || editing.builtin}/>
            </div>
            <div class="row">
                <label for="sk-name">{t("ai.settings.skills.label.name")}</label>
                <input id="sk-name" type="text" bind:value={editing.name} disabled={editing.builtin}
                       placeholder={t("ai.settings.skills.placeholder.name")}/>
            </div>
            <div class="row">
                <label for="sk-desc">{t("ai.settings.skills.label.description")}</label>
                <input id="sk-desc" type="text" bind:value={editing.description} disabled={editing.builtin}
                       placeholder={t("ai.settings.skills.placeholder.desc")}/>
            </div>
            <div class="row">
                <label for="sk-content">{t("ai.settings.skills.label.system_prompt")}</label>
                <textarea id="sk-content" bind:value={editing.content} disabled={editing.builtin}
                          rows="20"
                          placeholder={t("ai.settings.skills.placeholder.content")}></textarea>
            </div>
            <div class="actions">
                {#if !editing.builtin}
                    <button class="btn btn-accent btn-sm" onclick={saveSkill} disabled={savingSkill}>
                        {savingSkill ? t("ai.settings.btn.saving") : t("common.save")}
                    </button>
                {/if}
                {#if !editing.builtin && !isNew}
                    <button class="btn btn-sm btn-danger" class:confirming={confirmingDelete}
                            onclick={() => editing && removeSkill(editing)}>
                        {confirmingDelete ? t("ai.settings.skills.btn.delete_confirm") : t("ai.settings.skills.btn.delete")}
                    </button>
                {/if}
                <button class="btn btn-sm" onclick={cancelEdit}>{editing.builtin ? t("ai.settings.skills.btn.back") : t("ai.settings.skills.btn.cancel")}</button>
            </div>
        </div>
    {/if}
</div>

<style>
    .page {
        padding: 24px;
        display: flex;
        flex-direction: column;
        gap: 12px;
    }

    .warn {
        display: flex;
        align-items: flex-start;
        gap: 8px;
        background: color-mix(in srgb, var(--warning) 12%, var(--bg));
        border-left: 3px solid var(--warning);
        padding: 8px 12px;
        border-radius: 4px;
        font-size: 12px;
        color: var(--text-sub);
        line-height: 1.5;
    }
    .warn a { color: var(--accent); }

    .form {
        display: flex;
        flex-direction: column;
        gap: 10px;
    }
    .row {
        display: flex;
        flex-direction: column;
        gap: 4px;
    }
    .row label {
        font-size: 12px;
        color: var(--text-sub);
    }
    .row input[type="text"],
    .row input[type="password"],
    .row select,
    .row textarea {
        width: 100%;
        box-sizing: border-box;
    }
    .row textarea {
        font-family: var(--term-font);
        font-size: 12px;
        resize: vertical;
        min-height: 240px;
    }
    /* Provider 列表行：激活单选照搬 ShellSettings 的 radio 骨架（隐藏 .radio-state +
       label 内 .shell-radio-indicator，indicator 视觉走全局样式）+ 操作。 */
    .provider-row {
        display: flex;
        justify-content: space-between;
        align-items: center;
        gap: 12px;
        padding: 10px 0;
        border-bottom: 1px solid var(--divider);
    }
    .provider-row:last-of-type { border-bottom: none; }
    .provider-info {
        position: relative;
        display: flex;
        align-items: center;
        gap: 10px;
        min-width: 0;
        flex: 0 1 auto;
        width: fit-content;
    }
    /* 隐藏但可聚焦的真 input —— 同 ShellSettings .radio-state。pointer-events:none
       让点击穿透到 label，label[for] 转发 focus。 */
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
        /* min-width:0 lets the label shrink below its text's min-content so
           the ellipsis on .provider-text can engage (flex items default to
           min-width:auto and refuse to shrink). */
        min-width: 0;
        /* 压掉全局 label 样式（11px/大写/600）——provider 名称与 URL 保持原大小写。 */
        font-size: inherit;
        font-weight: 400;
        text-transform: none;
        letter-spacing: normal;
        color: var(--text);
    }
    .radio-state:checked ~ .radio-label .provider-name { color: var(--accent); }
    .provider-text { min-width: 0; }
    .provider-name {
        font-weight: 600;
        font-size: 14px;
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .provider-sub {
        font-size: 12px;
        color: var(--text-sub);
        font-family: var(--term-font);
        overflow: hidden;
        text-overflow: ellipsis;
        white-space: nowrap;
    }
    .provider-actions {
        display: flex;
        align-items: center;
        gap: 10px;
        flex-shrink: 0;
    }
    .active-badge {
        font-size: 10px;
        padding: 1px 6px;
        border-radius: 3px;
        background: var(--accent-soft);
        color: var(--accent);
        font-weight: 500;
    }
    @media (max-width: 640px) {
        .provider-row { align-items: flex-start; flex-direction: column; }
        /* Column + flex-start shrink-wraps .provider-info (base width:
           fit-content), unbounding the ellipsis chain — stretch it back to
           the row width so long protocol · endpoint · model lines truncate. */
        .provider-info { width: 100%; }
        .provider-actions { align-self: flex-end; }
    }

    .actions {
        display: flex;
        gap: 8px;
        align-items: center;
        margin-top: 4px;
    }
    .btn-danger.confirming {
        animation: confirmPulse 1.2s ease-in-out infinite;
    }
    @keyframes confirmPulse {
        0%, 100% { box-shadow: 0 0 0 0 color-mix(in srgb, var(--error) 45%, transparent); }
        50%      { box-shadow: 0 0 0 6px color-mix(in srgb, var(--error) 0%, transparent); }
    }
    .note {
        font-size: 12px;
        color: var(--accent);
    }

    .skill-header {
        display: flex;
        align-items: center;
        justify-content: space-between;
        padding-right: 0;
    }

    /* Provider / Danger 卡片：复用全局 .card.surface-raised 提供 bg + 阴影 + 圆角，
       本地只加 padding + 内布局，跟 SyncScreen 同款。 */
    .provider-card,
    .danger-card,
    .list-card {
        padding: 18px;
        display: flex;
        flex-direction: column;
        gap: 14px;
    }

    /* 卡片内顶部行：说明在左、操作按钮（如脱敏规则的"新建"）在右。 */
    .card-head {
        display: flex;
        align-items: flex-start;
        justify-content: space-between;
        gap: 12px;
    }
    .card-head .hint {
        flex: 1;
    }

    /* 主开关行：title/desc 在左，switch 在右；不再依赖全局 .switch-card 容器。 */
    .danger-head {
        display: flex;
        align-items: center;
        gap: 12px;
    }
    .danger-head-body {
        flex: 1;
        display: flex;
        flex-direction: column;
        gap: 4px;
    }
    .danger-title {
        font-size: 13px;
        font-weight: 600;
        color: var(--text);
        text-transform: uppercase;
        letter-spacing: 0.04em;
    }
    /* "开启"时压成 error 红 —— 让危险态视觉无法忽视，防止用户开了忘了又跑命令。 */
    .danger-title.on { color: var(--error); }
    .danger-desc {
        font-size: 11px;
        color: var(--text-dim);
        line-height: 1.5;
    }
    .danger-err {
        font-size: 12px;
        color: var(--error);
    }

    /* 卡片内分隔线：用负边距贯穿到卡片左右边缘，视觉上是"横切"而非缩进线。 */
    .card-divider {
        height: 1px;
        background: var(--divider);
        margin: 2px -18px;
    }

    /* per-tool 自动批准 —— 嵌在 .danger-card 内，不再有自己的 bg/border。 */
    .auto-group {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }
    .auto-group.disabled {
        opacity: 0.5;
    }
    .auto-group-title {
        font-size: 11px;
        color: var(--text-sub);
        text-transform: uppercase;
        letter-spacing: 0.04em;
        margin-bottom: 4px;
    }
    .auto-row {
        display: flex;
        align-items: center;
        gap: 8px;
        font-size: 13px;
        cursor: pointer;
    }
    .auto-row input[type="checkbox"] {
        cursor: pointer;
    }
    .auto-group.disabled .auto-row,
    .auto-group.disabled .auto-row input[type="checkbox"] {
        cursor: not-allowed;
    }

    .banner {
        display: flex;
        align-items: center;
        gap: 8px;
        padding: 6px 12px;
        background: color-mix(in srgb, var(--error) 12%, var(--bg));
        color: var(--error);
        border-radius: 4px;
        font-size: 12px;
    }
    .banner-close {
        margin-left: auto;
        background: transparent;
        border: none;
        color: inherit;
        font-size: 14px;
        cursor: pointer;
    }

    .skill-list {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }
    .skill-item {
        text-align: left;
        padding: 10px 14px;
        border: none;
        background: var(--bg);
        cursor: pointer;
        font-family: inherit;
        color: var(--text);
        transition: box-shadow 0.13s;
    }
    .skill-item:hover { box-shadow: var(--raised); }
    .skill-row {
        display: flex;
        align-items: baseline;
        gap: 8px;
    }
    .skill-name {
        font-weight: 600;
        font-size: 13px;
    }
    .skill-tag {
        font-size: 10px;
        padding: 1px 6px;
        border-radius: 3px;
        font-weight: 500;
    }
    .skill-tag.builtin {
        background: var(--surface);
        color: var(--text-dim);
    }
    .skill-tag.user {
        background: var(--accent-soft);
        color: var(--accent);
    }
    .skill-id {
        font-family: var(--term-font);
        font-size: 11px;
        color: var(--text-dim);
        margin-left: auto;
    }
    .skill-desc {
        font-size: 12px;
        color: var(--text-sub);
        margin-top: 4px;
    }

    .placeholder {
        text-align: center;
        padding: 24px;
        color: var(--text-dim);
        font-size: 13px;
    }

    /* 脱敏规则 / 命令黑名单 ────────────────────────────────── */
    /* 卡片内说明文本：跟 ShellSettings .shell-hint 同档（11px / dim / 1.5）。 */
    .hint {
        font-size: 11px;
        color: var(--text-dim);
        line-height: 1.5;
    }

    /* 可用性过滤：照 ShellSettings 的 tips-group / tips-title / tips-list。 */
    .tips-group {
        display: flex;
        flex-direction: column;
        gap: 6px;
    }
    .tips-title {
        font-size: 11px;
        font-weight: 600;
        color: var(--text-sub);
        text-transform: uppercase;
        letter-spacing: 0.5px;
    }
    .tips-list {
        margin: 0;
        padding-left: 18px;
        font-size: 12px;
        color: var(--text);
        line-height: 1.6;
    }
    .tips-list li {
        margin: 2px 0;
    }
    /* 列表行：pattern → replacement，等宽字体，过长截断不撑破卡片。 */
    .rule-line {
        display: flex;
        align-items: baseline;
        gap: 8px;
        min-width: 0;
    }
    .rule-pattern,
    .rule-replacement {
        font-family: var(--term-font);
        font-size: 12px;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }
    .rule-pattern {
        color: var(--text);
        flex: 1 1 auto;
    }
    .rule-arrow {
        color: var(--text-dim);
        flex: 0 0 auto;
    }
    .rule-replacement {
        color: var(--accent);
        flex: 0 1 auto;
    }
    /* pattern / replacement 输入框用等宽，跟正则语义一致。 */
    .row input.mono {
        font-family: var(--term-font);
    }

    /* 命令黑名单：分类行（标签 + 命令列表），整类编辑。 */
    .bl-row {
        display: flex;
        align-items: baseline;
        gap: 10px;
        min-width: 0;
    }
    .bl-cat {
        flex: 0 0 auto;
        font-size: 12px;
        color: var(--text);
        font-weight: 600;
    }
    .bl-cmds {
        flex: 1 1 auto;
        font-family: var(--term-font);
        font-size: 12px;
        color: var(--text-dim);
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
    }
    .bl-empty {
        font-style: italic;
        opacity: 0.7;
    }
    /* 命令列表短，覆盖 `.row textarea` 的 240px（那是给 skill 正文的）。
       选择器带 .row + 元素，特异性高于 `.row textarea` 才压得住。 */
    .row textarea.bl-textarea {
        min-height: 3.2rem;
        font-size: 12px;
        line-height: 1.5;
    }
</style>
