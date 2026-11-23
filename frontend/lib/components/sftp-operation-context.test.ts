import { readFileSync } from "node:fs";
import { join } from "node:path";
import { runInNewContext } from "node:vm";
import { describe, expect, it, vi } from "vitest";
import ts from "typescript";

const component = readFileSync(join(process.cwd(), "frontend/lib/components/SftpBrowser.svelte"), "utf8");
const script = component.slice(component.indexOf('<script lang="ts">') + '<script lang="ts">'.length, component.indexOf("</script>"));
const parsed = ts.createSourceFile("SftpBrowser.ts", script, ts.ScriptTarget.Latest, true);
const functionNames = new Set([
  "transferContext", "queueDownloads", "downloadEntry", "downloadSelected", "uploadName",
  "uploadFiles", "uploadFolder", "uploadFile",
]);
const functions = parsed.statements.filter((node) => ts.isFunctionDeclaration(node) && node.name && functionNames.has(node.name.text));
const code = ts.transpileModule(functions.map((node) => node.getText(parsed)).join("\n"), {
  compilerOptions: { target: ts.ScriptTarget.ES2022 },
}).outputText;

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

function readyPaths(_root: string, paths: string[]) {
  return paths.map((relative_path) => ({ status: "ready", relative_path, location: `/local/${relative_path}` }));
}

function load() {
  const pickFiles = vi.fn();
  const pickFile = vi.fn();
  const pickDirectory = vi.fn();
  const walkDirectory = vi.fn();
  const resolvePaths = vi.fn(async (root: string, paths: string[]) => readyPaths(root, paths));
  const startUpload = vi.fn().mockResolvedValue("upload");
  const startDownload = vi.fn().mockResolvedValue("download");
  const invoke = vi.fn();
  const state = {
    meta: { sessionId: "ssh-original" }, cwd: "/original", sftpId: "sftp-original",
    entries: [] as { name: string; is_dir: boolean; size: number }[], selected: new Set<string>(),
    error: "", notice: "", closeCtxMenu: vi.fn(),
    fileAccess: { pickFiles, pickFile, pickDirectory, walkDirectory, resolvePaths },
    transfers: { startUpload, startDownload }, invoke,
    joinRemote: (...parts: string[]) => parts.join("/"),
    errMsg: (error: unknown) => error instanceof Error ? error.message : String(error),
    t: (key: string) => key,
  };
  const actions = runInNewContext(code + "\n({ uploadFiles, uploadFolder, uploadFile, downloadEntry, downloadSelected })", state);
  function navigate(suffix: string) {
    state.meta = { sessionId: `ssh-${suffix}` };
    state.cwd = `/directory-${suffix}`;
    state.sftpId = `sftp-${suffix}`;
  }
  return { state, actions, navigate, pickFiles, pickFile, pickDirectory, walkDirectory, resolvePaths, startUpload, startDownload, invoke };
}

const files = [{ location: "/source/first.txt", name: "first.txt" }, { location: "/source/second.txt", name: "second.txt" }];

describe("SFTP operation destination context", () => {
  it("keeps multi-file uploads on the original connection and directory through picker and registration waits", async () => {
    const h = load();
    const picker = deferred<typeof files>();
    const registration = deferred<string>();
    h.pickFiles.mockReturnValue(picker.promise);
    h.startUpload.mockReturnValueOnce(registration.promise);

    const operation = h.actions.uploadFiles();
    h.navigate("during-picker");
    picker.resolve(files);
    await vi.waitFor(() => expect(h.startUpload).toHaveBeenCalledTimes(1));
    h.navigate("during-registration");
    registration.resolve("registered");
    await operation;

    expect(h.startUpload.mock.calls.map(([args]) => args)).toEqual(files.map((file) => ({
      sessionId: "ssh-original", localPath: file.location, remotePath: `/original/${file.name}`,
    })));
  });

  it("keeps folder uploads on the original destination while picking, walking and registering files", async () => {
    const h = load();
    const picker = deferred<{ location: string; name: string }>();
    const walk = deferred<{ local_path: string; rel_path: string; size: number }[]>();
    const registration = deferred<string>();
    h.pickDirectory.mockReturnValue(picker.promise);
    h.walkDirectory.mockReturnValue(walk.promise);
    h.startUpload.mockReturnValueOnce(registration.promise);

    const operation = h.actions.uploadFolder();
    h.navigate("during-picker");
    picker.resolve({ location: "/source/project", name: "project" });
    await vi.waitFor(() => expect(h.walkDirectory).toHaveBeenCalledWith("/source/project"));
    h.navigate("during-walk");
    walk.resolve(files.map((file) => ({ local_path: file.location, rel_path: `sub/${file.name}`, size: 42 })));
    await vi.waitFor(() => expect(h.startUpload).toHaveBeenCalledTimes(1));
    h.navigate("during-registration");
    registration.resolve("registered");
    await operation;

    expect(h.startUpload.mock.calls.map(([args]) => args)).toEqual(files.map((file) => ({
      sessionId: "ssh-original", localPath: file.location, remotePath: `/original/project/sub/${file.name}`,
    })));
  });

  it("keeps the single-file upload fallback bound before the picker opens", async () => {
    const h = load();
    const picker = deferred<(typeof files)[number]>();
    h.pickFile.mockReturnValue(picker.promise);
    const operation = h.actions.uploadFile();
    h.navigate("during-picker");
    picker.resolve(files[0]);
    await operation;
    expect(h.startUpload).toHaveBeenCalledWith({
      sessionId: "ssh-original", localPath: files[0].location, remotePath: "/original/first.txt",
    });
  });

  it("keeps a context-menu download bound before authorizing the local folder", async () => {
    const h = load();
    const picker = deferred<{ location: string; name: string }>();
    h.pickDirectory.mockReturnValue(picker.promise);
    const operation = h.actions.downloadEntry({ name: "report.txt", is_dir: false, size: 42 });
    h.navigate("during-picker");
    picker.resolve({ location: "/downloads", name: "Downloads" });
    await operation;
    expect(h.startDownload).toHaveBeenCalledWith({
      sessionId: "ssh-original", localPath: "/local/report.txt", remotePath: "/original/report.txt", sizeHint: 42,
    });
  });

  it("keeps selected downloads on their original SSH channel and root during remote walks and local resolution", async () => {
    const h = load();
    const picker = deferred<{ location: string; name: string }>();
    const walk = deferred<{ rel_path: string; size: number }[]>();
    const resolution = deferred<ReturnType<typeof readyPaths>>();
    h.state.entries = [
      { name: "first-tree", is_dir: true, size: 0 },
      { name: "second-tree", is_dir: true, size: 0 },
      { name: "tail.txt", is_dir: false, size: 42 },
    ];
    h.state.selected = new Set(h.state.entries.map((entry) => entry.name));
    h.pickDirectory.mockReturnValue(picker.promise);
    h.invoke.mockReturnValueOnce(walk.promise).mockResolvedValue([]);
    h.resolvePaths.mockReturnValueOnce(resolution.promise);

    const operation = h.actions.downloadSelected();
    h.navigate("during-picker");
    picker.resolve({ location: "/downloads", name: "Downloads" });
    await vi.waitFor(() => expect(h.invoke).toHaveBeenCalledTimes(1));
    h.navigate("during-walk");
    walk.resolve([{ rel_path: "nested.txt", size: 21 }]);
    await vi.waitFor(() => expect(h.resolvePaths).toHaveBeenCalledTimes(1));
    h.navigate("during-resolution");
    resolution.resolve(readyPaths("/downloads", ["first-tree/nested.txt"]));
    await operation;

    expect(h.invoke.mock.calls).toEqual([
      ["sftp_walk_remote_dir", { sftpId: "sftp-original", remoteRoot: "/original/first-tree" }],
      ["sftp_walk_remote_dir", { sftpId: "sftp-original", remoteRoot: "/original/second-tree" }],
    ]);
    expect(h.startDownload.mock.calls.map(([args]) => [args.sessionId, args.remotePath])).toEqual([
      ["ssh-original", "/original/first-tree/nested.txt"], ["ssh-original", "/original/tail.txt"],
    ]);
  });

  it("shows an expired original connection error instead of registering remaining uploads on a replacement host", async () => {
    const h = load();
    const picker = deferred<typeof files>();
    h.pickFiles.mockReturnValue(picker.promise);
    h.startUpload.mockImplementation(async (args) => {
      if (args.sessionId === "ssh-original") throw new Error("Original SSH session closed");
      return "registered-on-new-host";
    });
    const operation = h.actions.uploadFiles();
    h.navigate("replacement-host");
    picker.resolve(files);
    await operation;

    expect(h.startUpload).toHaveBeenCalledTimes(1);
    expect(h.startUpload.mock.calls[0][0].sessionId).toBe("ssh-original");
    expect(h.state.error).toBe("Original SSH session closed");
    expect(h.state.notice).toBe("");
  });
});
