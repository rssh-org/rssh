import { invoke } from "@tauri-apps/api/core";

export interface ReportSummary {
  id: string;
  title: string;
  createdAt: number;
}

interface ReportIndexItem extends ReportSummary {
  isCurrent: boolean;
}

export interface TerminalExcerpt {
  sessionId: string;
  sessionName: string;
  text: string;
}

export interface IncidentReport extends ReportSummary {
  content: string;
}

let _reports = $state.raw<ReportIndexItem[]>([]);
let pending = Promise.resolve();

export function reports(): ReportSummary[] { return _reports; }
export function currentReport(): ReportSummary | null {
  return _reports.find((report) => report.isCurrent) ?? null;
}

// Apply reads and writes in dispatch order, including rapid captures on different tabs.
// The host returns the index from the same transaction that committed the edit.
function queryIndex(command: string, args?: Record<string, unknown>): Promise<ReportIndexItem[]> {
  const request = pending.then(async () => {
    const index = await invoke<ReportIndexItem[]>(command, args);
    _reports = index;
    return index;
  });
  pending = request.then(() => {}, () => {});
  return request;
}

export async function refreshReports(): Promise<void> {
  await queryIndex("list_incident_reports");
}

export function loadReport(id: string): Promise<IncidentReport> {
  return pending.then(() => invoke<IncidentReport>("load_incident_report", { id }));
}

export async function selectReport(id: string): Promise<void> {
  await queryIndex("select_incident_report", { id });
}

export async function updateReport(id: string, content: string, previousContent: string): Promise<void> {
  await queryIndex("update_incident_report", { id, content, previousContent });
}

export async function deleteReport(id: string): Promise<void> {
  await queryIndex("delete_incident_report", { id });
}

export async function captureExcerpt(excerpt: TerminalExcerpt, target?: string): Promise<ReportSummary> {
  const now = new Date();
  const month = String(now.getMonth() + 1).padStart(2, "0");
  const day = String(now.getDate()).padStart(2, "0");
  const index = await queryIndex("capture_incident_excerpt", {
    excerpt,
    defaultTitle: `${now.getFullYear()}-${month}-${day} ${excerpt.sessionName}`,
    target: target === undefined ? "current" : target === "new" ? "new" : { existing: target },
  });
  return index.find((report) => report.isCurrent)!;
}

/** Duplicate titles can be created on the same day or by renaming reports. */
export function reportLabel(report: ReportSummary, allReports: ReportSummary[]): string {
  const duplicates = allReports.filter((other) => other.title === report.title && other.id !== report.id);
  if (!duplicates.length) return report.title;
  let length = 8;
  while (length < report.id.length && duplicates.some((other) => other.id.startsWith(report.id.slice(0, length)))) {
    length++;
  }
  return `${report.title} · ${report.id.slice(0, length)}`;
}
