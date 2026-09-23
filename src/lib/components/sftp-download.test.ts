import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { runInNewContext } from "node:vm";
import ts from "typescript";

const source = readFileSync(join(process.cwd(), "src/lib/components/SftpBrowser.svelte"), "utf8");

function functionSource(name: string, nextName: string): string {
  const start = source.indexOf(`async function ${name}`);
  const end = source.indexOf(`function ${nextName}`, start);
  if (start < 0 || end < 0) throw new Error(`missing ${name} function boundary`);
  return source.slice(start, end);
}

describe("SFTP context-menu download", () => {
  it("picks a target path and queues the file through the transfer store", () => {
    const downloadEntry = functionSource("downloadEntry", "confirmDelete");

    expect(downloadEntry).toContain("fileAccess.pickSavePath(entry.name)");
    expect(downloadEntry).toContain("transfers.startDownload");
    expect(downloadEntry).not.toContain('"sftp_save_file"');
  });
});


describe("SFTP batch download isolation", () => {
  it("queues valid files around a failed target and reports that target by name", async () => {
    const start = source.indexOf("async function queueDownloads");
    const end = source.indexOf("async function downloadEntry", start);
    const startDownload = vi.fn().mockResolvedValue(undefined);
    const resolvePaths = vi.fn().mockResolvedValue([
      { status: "ready", relative_path: "good.txt", location: "file://docs/good.txt" },
      { status: "failed", relative_path: "report.txt", error: "target is a directory" },
      { status: "ready", relative_path: "中文 %2F.txt", location: "file://docs/%E4%B8%AD%E6%96%87%20%252F.txt" },
    ]);
    const code = ts.transpileModule(source.slice(start, end), {
      compilerOptions: { target: ts.ScriptTarget.ES2022 },
    }).outputText;
    const queueDownloads = runInNewContext(code + "\nqueueDownloads", {
      cwd: "/remote", sftpId: "sftp-test", meta: { sessionId: "ssh-test" },
      joinRemote: (dir: string, name: string) => `${dir}/${name}`,
      fileAccess: { resolvePaths }, transfers: { startDownload }, errMsg: String,
    }) as (files: { name: string; is_dir: boolean; size: number }[], root: string) => Promise<unknown>;
    const items = ["good.txt", "report.txt", "中文 %2F.txt"].map((name) => ({ name, is_dir: false, size: 42 }));
    await expect(queueDownloads(items, "file://docs/root")).resolves.toEqual({
      queued: 2, walkErrors: ["report.txt: target is a directory"],
    });
    expect(startDownload.mock.calls.map(([file]) => file.remotePath)).toEqual([
      "/remote/good.txt", "/remote/中文 %2F.txt",
    ]);
    expect(startDownload.mock.calls[1][0].localPath).toBe("file://docs/%E4%B8%AD%E6%96%87%20%252F.txt");
  });
});
