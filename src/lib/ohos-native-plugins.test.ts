import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import * as nodeFs from "node:fs";
import * as fs from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import { describe, expect, it, onTestFinished, vi } from "vitest";

interface TypedValue { typeName: string; value: unknown }
interface Plugin {
  id: string;
  requires: string[];
  attachContext?(context: ReturnType<typeof callContext>): void;
  onDispose?(): Promise<void>;
  invokeAsync(action: string, value: TypedValue, context: ReturnType<typeof callContext>): Promise<TypedValue>;
}

function loadPlugin(name: string, modules: Record<string, unknown>, globals: Record<string, unknown> = {}): Plugin {
  const path = fileURLToPath(new URL(
    `../../src-tauri/gen/ohos/entry/src/main/ets/plugins/${name}.ets`, import.meta.url,
  ));
  const code = ts.transpileModule(readFileSync(path, "utf8"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  const exports = {} as Record<string, new () => Plugin>;
  runInNewContext(code, {
    ...globals,
    exports,
    require: (id: string) => {
      if (id === "@ohos-rs/ability") return { AsyncPluginBase: class {
        private context?: ReturnType<typeof callContext>;
        attachContext(context: ReturnType<typeof callContext>) { this.context = context; }
        getContext() { return this.context; }
      } };
      if (!(id in modules)) throw new Error(`Unexpected platform dependency: ${id}`);
      return modules[id];
    },
  }, { filename: path });
  return new exports[name]();
}

function callContext() {
  return {
    isActive: vi.fn(() => true),
    isCancelled: vi.fn(() => false),
    invokeNativeSync: vi.fn(),
    onCancel: vi.fn((_callback: () => void) => () => {}),
  };
}

function filesHarness() {
  const file = { fd: 42 };
  const openSync = vi.fn(() => file);
  const closeSync = vi.fn();
  const OpenMode = { READ_ONLY: 0, WRITE_ONLY: 1, CREATE: 64, TRUNC: 1024 };
  const plugin = loadPlugin("FilesAccessPlugin", {
    "@ohos.file.fs": { default: { openSync, closeSync, OpenMode } },
    "@ohos.file.fileuri": { default: {} },
  });
  const context = callContext();
  const external = Object.freeze({ opaqueNativeFile: true });
  context.invokeNativeSync.mockReturnValue(external);
  const request = (write = false): TypedValue => ({
    typeName: "rssh.files.OpenRequest", value: { uri: "file://docs/document.txt", write, createInDirectory: false },
  });
  return { plugin, context, external, request, file, openSync, closeSync, OpenMode };
}

function clipboardHarness() {
  const getData = vi.fn().mockResolvedValue({ getPrimaryText: () => "text" });
  const setData = vi.fn().mockResolvedValue(undefined);
  const createData = vi.fn((_type, text) => ({ text }));
  const plugin = loadPlugin("ClipboardPlugin", {
    "@ohos.pasteboard": {
      default: { getSystemPasteboard: () => ({ getData, setData }), createData, MIMETYPE_TEXT_PLAIN: "text/plain" },
    },
  });
  return { plugin, context: callContext(), getData, setData, createData };
}

describe("HarmonyOS typed file ownership plugin", () => {
  it.each([false, true])("duplicates on the UI callback before closing the borrowed file (write=%s)", async (write) => {
    const h = filesHarness();
    const order: string[] = [];
    h.openSync.mockImplementation(() => { order.push("open"); return h.file; });
    h.context.invokeNativeSync.mockImplementation(() => {
      order.push("duplicate");
      expect(h.closeSync).not.toHaveBeenCalled();
      return h.external;
    });
    h.closeSync.mockImplementation(() => { order.push("close"); });

    const result = await h.plugin.invokeAsync("open-file", h.request(write), h.context);

    expect(h.openSync).toHaveBeenCalledExactlyOnceWith(
      "file://docs/document.txt", write ? h.OpenMode.WRITE_ONLY | h.OpenMode.CREATE | h.OpenMode.TRUNC : h.OpenMode.READ_ONLY,
    );
    expect(h.context.invokeNativeSync).toHaveBeenCalledExactlyOnceWith(
      "duplicate-file", "rssh.files.BorrowedFile", "rssh.files.OpenedFile", { fd: h.file.fd },
    );
    expect(result.typeName).toBe("rssh.files.OpenedFile");
    expect(result.value).toBe(h.external);
    expect(h.closeSync).toHaveBeenCalledExactlyOnceWith(h.file);
    expect(order).toEqual(["open", "duplicate", "close"]);
  });

  it("closes the source file when native duplication fails", async () => {
    const h = filesHarness();
    h.context.invokeNativeSync.mockImplementation(() => { throw new Error("dup failed"); });

    await expect(h.plugin.invokeAsync("open-file", h.request(), h.context)).rejects.toThrow("dup failed");
    expect(h.closeSync).toHaveBeenCalledExactlyOnceWith(h.file);
  });

  it.each(["inactive", "cancelled"])("does not open a file for an %s call", async (state) => {
    const h = filesHarness();
    h.context.isActive.mockReturnValue(state !== "inactive");
    h.context.isCancelled.mockReturnValue(state === "cancelled");

    await expect(h.plugin.invokeAsync("open-file", h.request(), h.context)).rejects.toThrow();
    expect(h.openSync).not.toHaveBeenCalled();
  });

  it.each([
    { typeName: "std.string", value: "file://docs/a" },
    { typeName: "rssh.files.OpenRequest", value: null },
    { typeName: "rssh.files.OpenRequest", value: { uri: "", write: false } },
    { typeName: "rssh.files.OpenRequest", value: { uri: "file://docs/a", write: "false" } },
    { typeName: "rssh.files.OpenRequest", value: { uri: "file://docs/a", write: false, createInDirectory: true } },
  ])("rejects malformed typed requests before platform access", async (payload) => {
    const h = filesHarness();
    await expect(h.plugin.invokeAsync("open-file", payload, h.context)).rejects.toThrow();
    expect(h.openSync).not.toHaveBeenCalled();
  });
});

describe("HarmonyOS clipboard plugin", () => {
  it("reads and writes text with named values", async () => {
    const h = clipboardHarness();
    await expect(h.plugin.invokeAsync("read-text", { typeName: "rssh.clipboard.ReadRequest", value: {} }, h.context))
      .resolves.toEqual({ typeName: "rssh.clipboard.TextResponse", value: { text: "text" } });
    await expect(h.plugin.invokeAsync("write-text", {
      typeName: "rssh.clipboard.WriteRequest", value: { text: "hello" },
    }, h.context)).resolves.toEqual({ typeName: "rssh.clipboard.WriteResponse", value: {} });
    expect(h.createData).toHaveBeenCalledExactlyOnceWith("text/plain", "hello");
    expect(h.setData).toHaveBeenCalledExactlyOnceWith({ text: "hello" });
  });

  it.each(["inactive", "cancelled"])("rejects %s calls before reading or writing", async (state) => {
    const h = clipboardHarness();
    h.context.isActive.mockReturnValue(state !== "inactive");
    h.context.isCancelled.mockReturnValue(state === "cancelled");
    await expect(h.plugin.invokeAsync("read-text", { typeName: "rssh.clipboard.ReadRequest", value: {} }, h.context))
      .rejects.toThrow();
    expect(h.getData).not.toHaveBeenCalled();
  });

  it("discards clipboard data when its framework call is cancelled while reading", async () => {
    const h = clipboardHarness();
    h.getData.mockImplementation(async () => {
      h.context.isCancelled.mockReturnValue(true);
      return { getPrimaryText: () => "late text" };
    });
    await expect(h.plugin.invokeAsync("read-text", { typeName: "rssh.clipboard.ReadRequest", value: {} }, h.context))
      .rejects.toThrow();
  });

  it("validates the payload before writing", async () => {
    const h = clipboardHarness();
    await expect(h.plugin.invokeAsync("write-text", {
      typeName: "rssh.clipboard.WriteRequest", value: { text: 12 },
    }, h.context)).rejects.toThrow();
    expect(h.setData).not.toHaveBeenCalled();
  });
});


function directoryHarness() {
  const directory = mkdtempSync(path.join(tmpdir(), "rssh-files-access-"));
  onTestFinished(() => rmSync(directory, { recursive: true, force: true }));
  function nativeError(error: unknown) {
    const source = error as NodeJS.ErrnoException;
    const codes: Record<string, number> = { EEXIST: 13900015, ENOENT: 13900002 };
    return Object.assign(new Error(source.message), { code: codes[source.code || ""] || -1 });
  }
  const native = {
    lstat: async (file: string) => {
      try {
        const stat = await fs.lstat(file);
        return { size: stat.size, isFile: () => stat.isFile(), isDirectory: () => stat.isDirectory(), isSymbolicLink: () => stat.isSymbolicLink() };
      } catch (error) { throw nativeError(error); }
    },
    listFile: (file: string) => fs.readdir(file),
    mkdir: async (file: string) => { try { await fs.mkdir(file); } catch (error) { throw nativeError(error); } },
    OpenMode: { READ_ONLY: nodeFs.constants.O_RDONLY, WRITE_ONLY: nodeFs.constants.O_WRONLY, CREATE: nodeFs.constants.O_CREAT, TRUNC: nodeFs.constants.O_TRUNC },
    openSync: (location: string, mode: number) => {
      const filePath = location.startsWith("file://") ? fileURLToPath(location) : location;
      // The native docs URI route sends nonexistent files to DataShare, which
      // cannot create a directory child. Only the path route honors CREATE.
      if (location.startsWith("file://") && !nodeFs.existsSync(filePath)) {
        throw nativeError(Object.assign(new Error("No such file or directory"), { code: "ENOENT" }));
      }
      return { fd: nodeFs.openSync(filePath, mode) };
    },
    closeSync: (file: { fd: number }) => nodeFs.closeSync(file.fd),
  };
  const plugin = loadPlugin("FilesAccessPlugin", {
    "@ohos.file.fs": { default: native },
    "@ohos.file.fileuri": { default: { FileUri: class { path: string; constructor(uri: string) { this.path = fileURLToPath(uri); } } } },
  });
  const context = callContext();
  const uri = "file://" + directory;
  const invoke = (action: string, relativePaths: string[] = [], write = false) => plugin.invokeAsync(action, {
    typeName: "rssh.files.DirectoryRequest", value: { uri, relativePaths, write },
  }, context);
  return { directory, uri, plugin, context, native, invoke };
}

describe("HarmonyOS selected directory access", () => {
  it("preserves encoded URI names and creates only nested parents before download", async () => {
    const h = directoryHarness();
    const relative = "目录 空格/report #%?.txt";
    const result = await h.invoke("resolve-paths", [relative], true);
    const files = (result.value as { files: string[] }).files;
    expect(files[0]).toBe(h.uri + "/%E7%9B%AE%E5%BD%95%20%E7%A9%BA%E6%A0%BC/report%20%23%25%3F.txt");
    expect((await fs.stat(path.join(h.directory, "目录 空格"))).isDirectory()).toBe(true);
    await expect(fs.stat(path.join(h.directory, relative))).rejects.toMatchObject({ code: "ENOENT" });
    h.context.invokeNativeSync.mockImplementation((_action, _request, _response, file) => {
      nodeFs.writeSync(file.fd, "downloaded");
      return {};
    });
    await h.plugin.invokeAsync("open-file", {
      typeName: "rssh.files.OpenRequest", value: { uri: files[0], write: true, createInDirectory: true },
    }, h.context);
    expect(await fs.readFile(path.join(h.directory, relative), "utf8")).toBe("downloaded");
  });

  it("rejects relative traversal and symlink download destinations", async () => {
    const h = directoryHarness();
    for (const name of ["../outside", "/absolute", "./file", "x//file", "x\\file", "x\0file"]) {
      await expect(h.invoke("resolve-paths", [name], true)).rejects.toThrow("relative file name");
    }
    const outside = await fs.mkdtemp(path.join(tmpdir(), "rssh-files-outside-"));
    onTestFinished(() => rmSync(outside, { recursive: true, force: true }));
    await fs.symlink(outside, path.join(h.directory, "link"));
    await expect(h.invoke("resolve-paths", ["link/escape.txt"], true)).rejects.toThrow("symbolic link");
    await fs.writeFile(path.join(outside, "secret"), "unchanged");
    await fs.symlink(path.join(outside, "secret"), path.join(h.directory, "file-link"));
    await expect(h.invoke("resolve-paths", ["file-link"], true)).rejects.toThrow("regular file");
    expect(await fs.readFile(path.join(outside, "secret"), "utf8")).toBe("unchanged");
  });

  it("resolves several nested targets below an encoded selected directory", async () => {
    const h = directoryHarness();
    const selected = path.join(h.directory, "目录 #%");
    await fs.mkdir(selected);
    const uri = pathToFileURL(selected).toString();
    const names = ["first/second/third/a %.txt", "first/second/third/b #.txt"];
    const result = await h.plugin.invokeAsync("resolve-paths", {
      typeName: "rssh.files.DirectoryRequest", value: { uri, relativePaths: names, write: true },
    }, h.context);
    const files = (result.value as { files: string[] }).files;
    expect(files.map((file) => fileURLToPath(file))).toEqual(names.map((name) => path.join(selected, name)));
    expect(await fs.readdir(path.join(selected, "first/second/third"))).toEqual([]);
  });

  it("stops creating parent directories when cancellation arrives during mkdir", async () => {
    const h = directoryHarness();
    const mkdir = h.native.mkdir;
    h.native.mkdir = async (directory) => {
      await mkdir(directory);
      h.context.isCancelled.mockReturnValue(true);
    };
    await expect(h.invoke("resolve-paths", ["first/second/third/file.txt"], true)).rejects.toThrow("cancelled");
    expect(await fs.readdir(path.join(h.directory, "first"))).toEqual([]);
  });

  it("returns authorized source URIs with their relative names and skips symlinks", async () => {
    const h = directoryHarness();
    await fs.mkdir(path.join(h.directory, "sub"));
    await fs.writeFile(path.join(h.directory, "sub", "中文 #%.txt"), "payload");
    await fs.symlink(h.directory, path.join(h.directory, "cycle"));
    await fs.symlink(path.join(h.directory, "sub", "中文 #%.txt"), path.join(h.directory, "alias"));
    const result = await h.invoke("walk-directory");
    const entries = (result.value as { entries: { relativePath: string; size: number; uri: string }[] }).entries;
    expect(entries).toHaveLength(1);
    expect(entries[0].relativePath).toBe("sub/中文 #%.txt");
    expect(entries[0].size).toBe(7);
    expect(fileURLToPath(entries[0].uri)).toBe(path.join(h.directory, entries[0].relativePath));
  });

  it("stops directory work when the ability is destroyed or the call is cancelled", async () => {
    const h = directoryHarness();
    h.context.isActive.mockReturnValue(false);
    await expect(h.invoke("resolve-paths", ["a/b"], true)).rejects.toThrow("cancelled");
    expect(await fs.readdir(h.directory)).toEqual([]);
    h.context.isActive.mockReturnValue(true);
    h.native.listFile = async () => { h.context.isCancelled.mockReturnValue(true); return ["file"]; };
    await expect(h.invoke("walk-directory")).rejects.toThrow("cancelled");
  });

  it("validates source resolution without creating paths or following links", async () => {
    const h = directoryHarness();
    await fs.writeFile(path.join(h.directory, "source.txt"), "source");
    await fs.symlink(path.join(h.directory, "source.txt"), path.join(h.directory, "alias"));
    const resolved = await h.invoke("resolve-paths", ["source.txt"], false);
    expect((resolved.value as { files: string[] }).files).toEqual([h.uri + "/source.txt"]);
    await expect(h.invoke("resolve-paths", ["alias"], false)).rejects.toThrow("symbolic link");
    await expect(h.invoke("resolve-paths", ["missing/child"], false)).rejects.toThrow();
    expect(await fs.readdir(h.directory)).toEqual(["alias", "source.txt"]);
  });

  it("rejects a directory tree beyond the shared recursion limit", async () => {
    const h = directoryHarness();
    await fs.mkdir(path.join(h.directory, ...Array.from({ length: 32 }, () => "child")), { recursive: true });
    await expect(h.invoke("walk-directory")).rejects.toThrow("maximum depth of 32");
  });
});


function serialHarness(apiVersion = 26, capability = true) {
  const context = callContext();
  let read: ((data: Uint8Array) => void) | undefined;
  let disconnected: (() => void) | undefined;
  const port = {
    portInfo: { portName: "/dev/ttyACM0" },
    open: vi.fn(async (_config: unknown) => {}),
    close: vi.fn(async () => { read = undefined; disconnected = undefined; }),
    onDataRead: vi.fn((callback: (data: Uint8Array) => void) => { read = callback; }),
    onDisconnect: vi.fn((callback: () => void) => { disconnected = callback; }),
    write: vi.fn(async (data: Uint8Array, _timeout: number) => data.length),
    setDtr: vi.fn(async (_level: boolean) => {}),
    setRts: vi.fn(async (_level: boolean) => {}),
    sendBrk: vi.fn(async () => {}),
  };
  let moduleLoads = 0;
  const modules: Record<string, unknown> = {
    "@ohos.deviceInfo": { default: { sdkApiVersion: apiVersion } },
  };
  Object.defineProperty(modules, "@ohos.busManager.serial", { get: () => {
    moduleLoads++;
    return { default: { getSerialPortList: async () => [port] } };
  } });
  const plugin = loadPlugin("SerialPlugin", modules, { canIUse: () => capability });
  plugin.attachContext!(context);
  const invoke = (action: string, type: string, value: unknown) => plugin.invokeAsync(action, { typeName: `rssh.serial.${type}`, value }, context);
  const config = { id: "attempt-1", port: "/dev/ttyACM0", baudRate: 115200, dataBits: 8, stopBits: 1, parity: "none", flowControl: "hardware", xany: true };
  const open = () => invoke("open", "OpenRequest", config);
  const close = () => invoke("close", "PortRequest", { id: config.id });
  return { context, plugin, port, config, invoke, open, close, moduleLoads: () => moduleLoads, read: () => read, disconnected: () => disconnected };
}

describe("HarmonyOS public serial service", () => {
  it.each([[25, true], [26, false]])("never loads the API 26 module on unsupported devices (%s,%s)", async (version, capability) => {
    const h = serialHarness(version as number, capability as boolean);
    expect(h.moduleLoads()).toBe(0);
    await expect(h.invoke("list", "EmptyRequest", {})).rejects.toThrow("not supported");
    expect(h.moduleLoads()).toBe(0);
  });

  it("maps serial configuration and DTR, RTS, BREAK through the public port", async () => {
    const h = serialHarness();
    await h.open();
    expect(h.port.open).toHaveBeenCalledExactlyOnceWith({ baudRate: 115200, dataBits: 8, stopBits: 1, parity: "none", rtscts: true, xon: false, xoff: false, xany: true });
    await h.invoke("set-dtr", "LevelRequest", { id: h.config.id, level: false });
    await h.invoke("set-rts", "LevelRequest", { id: h.config.id, level: true });
    await h.invoke("send-break", "PortRequest", { id: h.config.id });
    expect(h.port.setDtr).toHaveBeenCalledWith(false);
    expect(h.port.setRts).toHaveBeenCalledWith(true);
    expect(h.port.sendBrk).toHaveBeenCalledOnce();
    await h.close();
  });

  it("forwards bytes under the open attempt ID and suppresses stale callbacks after closing", async () => {
    const h = serialHarness();
    await h.open();
    const read = h.read()!;
    read(new Uint8Array([0, 27, 255]));
    expect(h.context.invokeNativeSync).toHaveBeenCalledWith("serial-output", "rssh.serial.OutputEvent", "rssh.serial.EmptyResponse", { id: h.config.id, data: [0, 27, 255], closed: false });
    await h.close();
    read(new Uint8Array([1]));
    expect(h.context.invokeNativeSync).toHaveBeenCalledOnce();
    expect(h.port.close).toHaveBeenCalledOnce();
  });

  it("handles short writes and the platform 4096 byte limit without dropping bytes", async () => {
    const h = serialHarness();
    await h.open();
    const accepted: number[] = [];
    h.port.write.mockImplementation(async (data) => {
      expect(data.length).toBeLessThanOrEqual(4096);
      const written = Math.min(data.length, 1234);
      accepted.push(...data.slice(0, written));
      return written;
    });
    const bytes = Array.from({ length: 10000 }, (_, index) => index % 256);
    await h.invoke("write", "WriteRequest", { id: h.config.id, data: bytes });
    expect(accepted).toEqual(bytes);
    h.port.write.mockResolvedValue(0);
    await expect(h.invoke("write", "WriteRequest", { id: h.config.id, data: [1] })).rejects.toThrow("did not make progress");
    await h.close();
  });

  it.each(["close", "dispose", "cancel"])("closes a late authorized port after %s while authorization was pending", async (action) => {
    const h = serialHarness();
    let authorize!: () => void;
    h.port.open.mockImplementation(() => new Promise<void>((resolve) => { authorize = resolve; }));
    const opening = h.open();
    const rejected = expect(opening).rejects.toThrow("cancelled");
    await vi.waitFor(() => expect(h.port.open).toHaveBeenCalledOnce());
    if (action === "close") await h.close();
    else if (action === "dispose") await h.plugin.onDispose!();
    else h.context.onCancel.mock.calls[0][0]();
    authorize();
    await rejected;
    expect(h.port.close).toHaveBeenCalledOnce();
    expect(h.port.onDataRead).not.toHaveBeenCalled();
  });

  it("reports disconnect once and releases all callbacks during disposal", async () => {
    const h = serialHarness();
    await h.open();
    const disconnected = h.disconnected()!;
    disconnected();
    await vi.waitFor(() => expect(h.port.close).toHaveBeenCalledOnce());
    expect(h.context.invokeNativeSync).toHaveBeenCalledWith("serial-output", "rssh.serial.OutputEvent", "rssh.serial.EmptyResponse", { id: h.config.id, data: [], closed: true });
    disconnected();
    await h.plugin.onDispose!();
    expect(h.context.invokeNativeSync).toHaveBeenCalledOnce();
    expect(h.port.close).toHaveBeenCalledOnce();
  });

  it("retains a failed close for teardown without allowing further traffic", async () => {
    const h = serialHarness();
    await h.open();
    const read = h.read()!;
    h.port.close.mockRejectedValueOnce(new Error("temporary service failure"));
    await expect(h.close()).rejects.toThrow("temporary service failure");
    read(new Uint8Array([1]));
    expect(h.context.invokeNativeSync).not.toHaveBeenCalled();
    await expect(h.invoke("write", "WriteRequest", { id: h.config.id, data: [1] })).rejects.toThrow("closed");
    await h.plugin.onDispose!();
    expect(h.port.close).toHaveBeenCalledTimes(2);
    expect(h.read()).toBeUndefined();
  });

  it("waits for an in-flight close before completing teardown", async () => {
    const h = serialHarness();
    await h.open();
    let finishClose!: () => void;
    h.port.close.mockImplementation(() => new Promise<void>((resolve) => { finishClose = resolve; }));
    const closing = h.close();
    let disposed = false;
    const disposing = h.plugin.onDispose!().then(() => { disposed = true; });
    await Promise.resolve();
    expect(disposed).toBe(false);
    expect(h.port.close).toHaveBeenCalledOnce();
    finishClose();
    await Promise.all([closing, disposing]);
    expect(disposed).toBe(true);
  });
});
