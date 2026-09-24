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
  function loadQueueDownloads(mocks: { resolvePaths: ReturnType<typeof vi.fn>; invoke?: ReturnType<typeof vi.fn> }) {
    const start = source.indexOf("async function queueDownloads");
    const end = source.indexOf("async function downloadEntry", start);
    const startDownload = vi.fn().mockResolvedValue(undefined);
    const code = ts.transpileModule(source.slice(start, end), {
      compilerOptions: { target: ts.ScriptTarget.ES2022 },
    }).outputText;
    const queueDownloads = runInNewContext(code + "\nqueueDownloads", {
      cwd: "/remote", sftpId: "sftp-test", meta: { sessionId: "ssh-test" },
      joinRemote: (dir: string, name: string) => `${dir}/${name}`,
      fileAccess: { resolvePaths: mocks.resolvePaths }, invoke: mocks.invoke,
      transfers: { startDownload }, errMsg: String,
    }) as (files: { name: string; is_dir: boolean; size: number }[], root: string) => Promise<{ queued: number; walkErrors: string[] }>;
    return { queueDownloads, startDownload };
  }

  function readyPaths(_root: string, names: string[]) {
    return names.map((name) => ({ status: "ready", relative_path: name, location: `file://docs/${encodeURIComponent(name)}` }));
  }

  it("queues valid files around a failed target and reports that target by name", async () => {
    const resolvePaths = vi.fn().mockResolvedValue([
      { status: "ready", relative_path: "good.txt", location: "file://docs/good.txt" },
      { status: "failed", relative_path: "report.txt", error: "target is a directory" },
      { status: "ready", relative_path: "中文 %2F.txt", location: "file://docs/%E4%B8%AD%E6%96%87%20%252F.txt" },
    ]);
    const { queueDownloads, startDownload } = loadQueueDownloads({ resolvePaths });
    const items = ["good.txt", "report.txt", "中文 %2F.txt"].map((name) => ({ name, is_dir: false, size: 42 }));
    await expect(queueDownloads(items, "file://docs/root")).resolves.toEqual({
      queued: 2, walkErrors: ["report.txt: target is a directory"],
    });
    expect(startDownload.mock.calls.map(([file]) => file.remotePath)).toEqual([
      "/remote/good.txt", "/remote/中文 %2F.txt",
    ]);
    expect(startDownload.mock.calls[1][0].localPath).toBe("file://docs/%E4%B8%AD%E6%96%87%20%252F.txt");
  });

  it("queues a ready file before waiting for a slow directory walk", async () => {
    let finishWalk!: (files: { rel_path: string; size: number }[]) => void;
    let markWalkStarted!: () => void;
    const walk = new Promise<{ rel_path: string; size: number }[]>((resolve) => { finishWalk = resolve; });
    const walkStarted = new Promise<void>((resolve) => { markWalkStarted = resolve; });
    const invoke = vi.fn().mockImplementation(() => { markWalkStarted(); return walk; });
    const { queueDownloads, startDownload } = loadQueueDownloads({ resolvePaths: vi.fn(readyPaths), invoke });
    const result = queueDownloads([
      { name: "ready.txt", is_dir: false, size: 42 },
      { name: "slow-dir", is_dir: true, size: 0 },
    ], "file://docs/root");

    await walkStarted;
    try {
      expect(startDownload.mock.calls.map(([file]) => file.remotePath)).toEqual(["/remote/ready.txt"]);
    } finally {
      finishWalk([{ rel_path: "later.txt", size: 21 }]);
      await result;
    }
    expect(invoke).toHaveBeenCalledWith("sftp_walk_remote_dir", { sftpId: "sftp-test", remoteRoot: "/remote/slow-dir" });
    await expect(result).resolves.toEqual({ queued: 2, walkErrors: [] });
    expect(startDownload.mock.calls[1][0].remotePath).toBe("/remote/slow-dir/later.txt");
  });

  it("resolves large selections in bounded batches and isolates target failures across batches", async () => {
    const items = Array.from({ length: 200 }, (_, index) => ({ name: `${index}.txt`, is_dir: false, size: index }));
    const rejected = new Set(["0.txt", "100.txt", "199.txt"]);
    const resolvePaths = vi.fn().mockImplementation((root: string, names: string[]) => readyPaths(root, names).map((path) =>
      rejected.has(path.relative_path) ? { status: "failed", relative_path: path.relative_path, error: "denied" } : path,
    ));
    const { queueDownloads, startDownload } = loadQueueDownloads({ resolvePaths });

    await expect(queueDownloads(items, "file://docs/root")).resolves.toEqual({
      queued: 197, walkErrors: ["0.txt: denied", "100.txt: denied", "199.txt: denied"],
    });
    expect(resolvePaths.mock.calls.length).toBeGreaterThan(1);
    expect(resolvePaths.mock.calls.every(([, names]) => names.length <= 64)).toBe(true);
    expect(resolvePaths.mock.calls.flatMap(([, names]) => names)).toEqual(items.map((item) => item.name));
    expect(startDownload.mock.calls.map(([file]) => file.remotePath)).toEqual(
      items.filter((item) => !rejected.has(item.name)).map((item) => `/remote/${item.name}`),
    );
  });

  it("batches a walked subtree and still queues later selections after a failed subtree", async () => {
    const invoke = vi.fn().mockResolvedValueOnce(Array.from({ length: 130 }, (_, index) => ({ rel_path: `${index}.txt`, size: index })))
      .mockRejectedValueOnce(new Error("walk denied"));
    const resolvePaths = vi.fn(readyPaths);
    const { queueDownloads, startDownload } = loadQueueDownloads({ resolvePaths, invoke });
    await expect(queueDownloads([
      { name: "large-dir", is_dir: true, size: 0 },
      { name: "bad-dir", is_dir: true, size: 0 },
      { name: "tail.txt", is_dir: false, size: 3 },
    ], "file://docs/root")).resolves.toEqual({ queued: 131, walkErrors: ["bad-dir: Error: walk denied"] });
    expect(resolvePaths.mock.calls.every(([, names]) => names.length <= 64)).toBe(true);
    expect(startDownload.mock.calls[129][0].remotePath).toBe("/remote/large-dir/129.txt");
    expect(startDownload.mock.calls[130][0].remotePath).toBe("/remote/tail.txt");
  });
});
