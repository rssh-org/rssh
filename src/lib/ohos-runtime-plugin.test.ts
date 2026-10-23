import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import { describe, expect, it, vi } from "vitest";

const sourcePath = fileURLToPath(new URL(
  "../../src-tauri/gen/ohos/common/runtime/src/main/ets/plugins/RuntimePlugin.ets", import.meta.url,
));
const windowCapability = "SystemCapability.Window.SessionManager";
const folderCapability = "SystemCapability.FileManagement.UserFileService.FolderSelection";
const usbSerialCapability = "SystemCapability.USB.USBManager.Serial";
const busSerialCapability = "SystemCapability.BusManager.Serial";

function harness(deviceType: string, supported: string[], sdkApiVersion = 25) {
  const canIUse = vi.fn((capability: string) => supported.includes(capability));
  const modules: Record<string, unknown> = {
    "@ohos.deviceInfo": { default: { deviceType, sdkApiVersion } },
    "@ohos-rs/ability": { AsyncPluginBase: class {} },
  };
  type RuntimeModule = { RuntimePlugin: new () => {
    invokeAsync(action: string, payload: { typeName: string }, context: {
      isActive: () => boolean; isCancelled: () => boolean;
    }): Promise<unknown>;
  } };
  const context = { isActive: () => true, isCancelled: () => false };
  const cache = new Map<string, object>();
  function load(file: string): object {
    const cached = cache.get(file);
    if (cached) return cached;
    const exports = {};
    cache.set(file, exports);
    const code = ts.transpileModule(readFileSync(file, "utf8"), {
      compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
    }).outputText;
    runInNewContext(code, { exports, canIUse, require: (name: string) => {
      if (name.startsWith(".")) return load(path.resolve(path.dirname(file), `${name}.ets`));
      if (!(name in modules)) throw new Error(`Unexpected native dependency: ${name}`);
      return modules[name];
    } }, { filename: file });
    return exports;
  }
  const exports = load(sourcePath) as RuntimeModule;
  return { plugin: new exports.RuntimePlugin(), canIUse, context };
}

describe("HarmonyOS runtime capability bridge", () => {
  it("reports device shape and actual system capabilities independently", async () => {
    const h = harness("2in1", [windowCapability, folderCapability]);
    await expect(h.plugin.invokeAsync("device", { typeName: "rssh.runtime.DeviceRequest" }, h.context))
      .resolves.toEqual({ typeName: "rssh.runtime.DeviceResponse", value: {
        serial: false, deviceType: "2in1", multiWindow: true, folderSelection: true,
      } });
    expect(h.canIUse.mock.calls).toEqual([[usbSerialCapability], [windowCapability], [folderCapability]]);
  });

  it("does not infer native file or window support from being a PC", async () => {
    const h = harness("2in1", [windowCapability]);
    await expect(h.plugin.invokeAsync("device", { typeName: "rssh.runtime.DeviceRequest" }, h.context))
      .resolves.toEqual({ typeName: "rssh.runtime.DeviceResponse", value: {
        serial: false, deviceType: "2in1", multiWindow: true, folderSelection: false,
      } });
  });

  it.each([
    [18, [usbSerialCapability], false],
    [19, [], false],
    [19, [usbSerialCapability], true],
    [25, [busSerialCapability], false],
    [25, [usbSerialCapability], true],
    [26, [], false],
    [26, [busSerialCapability], true],
    [26, [usbSerialCapability], true],
  ] as Array<[number, string[], boolean]>)("advertises public serial support for API %i and matching system capabilities", async (version, supported, expected) => {
    // The harness rejects either native serial import. Capability discovery
    // must not load them, including on a phone below their supported API.
    const h = harness("2in1", supported, version);
    await expect(h.plugin.invokeAsync("device", { typeName: "rssh.runtime.DeviceRequest" }, h.context))
      .resolves.toMatchObject({ value: { serial: expected } });
  });

  it("preserves the phone device type", async () => {
    const h = harness("phone", []);
    await expect(h.plugin.invokeAsync("device", { typeName: "rssh.runtime.DeviceRequest" }, h.context))
      .resolves.toEqual({ typeName: "rssh.runtime.DeviceResponse", value: {
        serial: false, deviceType: "phone", multiWindow: false, folderSelection: false,
      } });
  });

  it.each([
    { isActive: () => false, isCancelled: () => false },
    { isActive: () => true, isCancelled: () => true },
  ])("rejects a detached or cancelled Ability before accessing device APIs", async (context) => {
    const h = harness("2in1", [windowCapability, folderCapability]);
    await expect(h.plugin.invokeAsync("device", { typeName: "rssh.runtime.DeviceRequest" }, context))
      .rejects.toThrow("cancelled");
    expect(h.canIUse).not.toHaveBeenCalled();
  });

  it.each([
    ["unknown", "rssh.runtime.DeviceRequest"],
    ["device", "wrong.Type"],
  ])("rejects a mismatched action or wire type (%s)", async (action, typeName) => {
    const h = harness("2in1", []);
    await expect(h.plugin.invokeAsync(action, { typeName }, h.context)).rejects.toThrow("Unsupported runtime");
  });
});
