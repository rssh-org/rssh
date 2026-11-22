import { errMsg, t } from "../i18n/index.svelte.ts";
import { captureExcerpt } from "../stores/incident-reports.svelte.ts";
import { toast } from "../stores/toast.svelte.ts";
import type { ReportExcerpt } from "./report-excerpt.ts";

export async function addToReport(excerpt: ReportExcerpt, target?: string): Promise<void> {
    try {
        const report = await captureExcerpt(excerpt, target);
        const chars = Array.from(excerpt.text.trim().replace(/\s+/gu, " "));
        const preview = chars.length <= 24
            ? chars.join("")
            : chars.slice(0, 12).join("") + "...." + chars.slice(-8).join("");
        toast.success(t("report.added", { preview, name: report.title }));
    } catch (error) {
        toast.error(errMsg(error));
    }
}
