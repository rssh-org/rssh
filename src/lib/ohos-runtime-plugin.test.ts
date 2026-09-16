import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import { describe, expect, it, vi } from "vitest";

const sourcePath = fileURLToPath(new URL(
  "../../src-tauri/gen/ohos/entry/src/main/ets/plugins/RuntimePlugin.ets", import.meta.url,
));
const code = ts.transpileModule(readFileSync(sourcePath, "utf8"), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
}).outputText;
const windowCapability = "SystemCapability.Window.SessionManager";
const folderCapability = "SystemCapability.FileManagement.UserFileService.FolderSelection";

function harness(deviceType: string, supported: string[], sdkApiVersion = 25) {
  const canIUse = vi.fn((capability: string) => supported.includes(capability));
  const modules: Record<string, unknown> = {
    "@ohos.deviceInfo": { default: { deviceType, sdkApiVersion } },
    "@ohos-rs/ability": { AsyncPluginBase: class {} },
  };
  const exports = {} as { RuntimePlugin: new () => {
    invokeAsync(action: string, payload: { typeName: string }, context: {
      isActive: () => boolean; isCancelled: () => boolean;
    }): Promise<unknown>;
  } };
  const context = { isActive: () => true, isCancelled: () => false };
  runInNewContext(code, { exports, canIUse, require: (name: string) => {
    if (!(name in modules)) throw new Error(`Unexpected native dependency: ${name}`);
    return modules[name];
  } }, { filename: sourcePath });
  return { plugin: new exports.RuntimePlugin(), canIUse, context };
}

describe("HarmonyOS runtime capability bridge", () => {
  it("reports device shape and actual system capabilities independently", async () => {
    const h = harness("2in1", [windowCapability, folderCapability]);
    await expect(h.plugin.invokeAsync("device", { typeName: "rssh.runtime.DeviceRequest" }, h.context))
      .resolves.toEqual({ typeName: "rssh.runtime.DeviceResponse", value: {
        serial: false, deviceType: "2in1", multiWindow: true, folderSelection: true,
      } });
    expect(h.canIUse.mock.calls).toEqual([[windowCapability], [folderCapability]]);
  });

  it("does not infer native file or window support from being a PC", async () => {
    const h = harness("2in1", [windowCapability]);
    await expect(h.plugin.invokeAsync("device", { typeName: "rssh.runtime.DeviceRequest" }, h.context))
      .resolves.toEqual({ typeName: "rssh.runtime.DeviceResponse", value: {
        serial: false, deviceType: "2in1", multiWindow: true, folderSelection: false,
      } });
  });

  it("requires the public API level and system support before advertising serial", async () => {
    const capability = "SystemCapability.BusManager.Serial";
    for (const [version, present, expected] of [[25, true, false], [26, false, false], [26, true, true]] as const) {
      const h = harness("2in1", present ? [capability] : [], version);
      await expect(h.plugin.invokeAsync("device", { typeName: "rssh.runtime.DeviceRequest" }, h.context))
        .resolves.toMatchObject({ value: { serial: expected } });
    }
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
