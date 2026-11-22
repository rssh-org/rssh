import { describe, expect, it, vi } from "vitest";
import { captureExcerpt, currentReport, deleteReport, refreshReports, reportLabel, reports, updateReport } from "./incident-reports.svelte";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

describe("incident reports", () => {
  it("creates a date-and-session title in local time and uses the saved first line in the index", async () => {
    const now = new Date(2026, 10, 21, 23, 30);
    vi.useFakeTimers();
    vi.setSystemTime(now);
    try {
      const report = { id: "r", title: "2026-11-21 gateway-01", createdAt: now.getTime(), isCurrent: true };
      invoke.mockResolvedValueOnce([report]);
      const excerpt = { sessionId: "gw", sessionName: "gateway-01", text: "  request\nbody  " };
      await captureExcerpt(excerpt);
      expect(invoke).toHaveBeenLastCalledWith("capture_incident_excerpt", {
        excerpt, defaultTitle: report.title, target: "current",
      });

      const edited = { ...report, title: "# Gateway timeout" };
      invoke.mockResolvedValueOnce([edited]);
      const original = report.title + "\n\nrequest";
      const content = edited.title + "\n\nEdited request\n";
      await updateReport(report.id, content, original);
      expect(invoke).toHaveBeenLastCalledWith("update_incident_report", {
        id: report.id, content, previousContent: original,
      });
      expect(currentReport()?.title).toBe(edited.title);
    } finally {
      vi.useRealTimers();
    }
  });

  it("distinguishes duplicate titles even when their short IDs match", () => {
    const a = { id: "123456781", title: "gateway", createdAt: 0 };
    const b = { ...a, id: "123456782" };
    expect(reportLabel(a, [a])).toBe("gateway");
    expect(reportLabel(a, [a, b])).not.toBe(reportLabel(b, [a, b]));
  });

  it("updates terminal capture destinations after deleting a report by ID", async () => {
    const first = { id: "a", title: "gateway", createdAt: 1, isCurrent: true };
    const second = { ...first, id: "b", isCurrent: false };
    invoke.mockResolvedValueOnce([first, second]);
    await refreshReports();

    invoke.mockResolvedValueOnce([{ ...second, isCurrent: true }]);
    await deleteReport(first.id);
    expect(invoke).toHaveBeenLastCalledWith("delete_incident_report", { id: "a" });
    expect(reports().map(report => report.id)).toEqual(["b"]);
    expect(currentReport()?.id).toBe("b");

    invoke.mockResolvedValueOnce([]);
    await deleteReport(second.id);
    expect(reports()).toEqual([]);
    expect(currentReport()).toBeNull();
  });
});
