import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import { describe, expect, it, vi } from "vitest";

interface TypedValue { typeName: string; value: unknown }
interface Plugin {
  id: string;
  requires: string[];
  invokeAsync(action: string, value: TypedValue, context: ReturnType<typeof callContext>): Promise<TypedValue>;
}

function loadPlugin(name: string, modules: Record<string, unknown>): Plugin {
  const path = fileURLToPath(new URL(
    `../../src-tauri/gen/ohos/entry/src/main/ets/plugins/${name}.ets`, import.meta.url,
  ));
  const code = ts.transpileModule(readFileSync(path, "utf8"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  const exports = {} as Record<string, new () => Plugin>;
  runInNewContext(code, {
    exports,
    require: (id: string) => {
      if (id === "@ohos-rs/ability") return { AsyncPluginBase: class {} };
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
  };
}

function filesHarness() {
  const file = { fd: 42 };
  const openSync = vi.fn(() => file);
  const closeSync = vi.fn();
  const OpenMode = { READ_ONLY: 0, WRITE_ONLY: 1, TRUNC: 1024 };
  const plugin = loadPlugin("FilesAccessPlugin", {
    "@ohos.file.fs": { default: { openSync, closeSync, OpenMode } },
  });
  const context = callContext();
  const external = Object.freeze({ opaqueNativeFile: true });
  context.invokeNativeSync.mockReturnValue(external);
  const request = (write = false): TypedValue => ({
    typeName: "rssh.files.OpenRequest", value: { uri: "file://docs/document.txt", write },
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
      "file://docs/document.txt", write ? h.OpenMode.WRITE_ONLY | h.OpenMode.TRUNC : h.OpenMode.READ_ONLY,
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
