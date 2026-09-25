import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import { describe, expect, it, vi } from "vitest";

const sourcePath = fileURLToPath(new URL(
  "../../src-tauri/gen/ohos/common/runtime/src/main/ets/ability/RsshAbility.ets", import.meta.url,
));
const code = ts.transpileModule(readFileSync(sourcePath, "utf8"), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
}).outputText;

function harness() {
  const applicationContext = { filesDir: "/isolated-test/base/files" };
  const context = { filesDir: "/isolated-test/base/haps/entry/files", getApplicationContext: () => applicationContext };
  const initContext = { basePath: context.filesDir, prefPath: context.filesDir, moduleName: "rssh_lib", resourceManager: {}, preferredLocales: "zh" };
  const createInitContext = vi.fn((_moduleName: string) => ({ ...initContext }));
  class NativeAbility {
    context = context;
    createInitContext(moduleName: string) { return createInitContext(moduleName); }
    onCreate() {}
    onWindowStageCreate() {}
    onWindowStageDestroy() {}
    onDestroy() {}
  }
  const modules: Record<string, unknown> = {
    "@ohos-rs/ability": {
      NativeAbility,
      LazyPlugin: class { constructor(public create: () => { id: string }) {} },
    },
  };
  for (const [pkg, cls, id] of [
    ["files", "FilesPlugin", "ohos.files"], ["url", "UrlPlugin", "ohos.url"],
    ["permission", "PermissionPlugin", "ohos.permission"], ["webview", "WebviewPlugin", "ohos.webview"],
    ["window", "WindowPlugin", "ohos.window"], ["app-control", "AppControlPlugin", "ohos.app-control"],
  ]) {
    modules[`@ohos-rs/ability-plugin-${pkg}`] = { [cls]: class { id = id; } };
  }
  modules["../plugins/FilesAccessPlugin"] = { FilesAccessPlugin: class { id = "rssh.files-access"; } };
  modules["../plugins/ClipboardPlugin"] = { ClipboardPlugin: class { id = "rssh.clipboard"; } };
  modules["../plugins/RuntimePlugin"] = { RuntimePlugin: class { id = "rssh.runtime"; } };
  modules["../plugins/SerialPlugin"] = { SerialPlugin: class { id = "rssh.serial"; } };
  const pluginExports = {};
  const pluginSource = sourcePath.replace('RsshAbility.ets', 'RsshPlugins.ets');
  const pluginCode = ts.transpileModule(readFileSync(pluginSource, "utf8"), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  runInNewContext(pluginCode, { exports: pluginExports, require: (name: string) => modules[name] });
  modules["./RsshPlugins"] = pluginExports;
  const exports = {} as { RsshAbility: new () => NativeAbility & { moduleName: string; bridgePlugins: { create: () => { id: string } }[] } };
  runInNewContext(code, { exports, require: (name: string) => {
    if (!(name in modules)) throw new Error(`Unexpected native dependency: ${name}`);
    return modules[name];
  } }, { filename: sourcePath });
  return { ability: new exports.RsshAbility(), NativeAbility, initContext, createInitContext, applicationContext };
}

describe("HarmonyOS framework host", () => {
  it("does not restore a raw database without its device-bound master key", () => {
    const config = JSON.parse(readFileSync(new URL(
      "../../src-tauri/gen/ohos/common/runtime/src/main/resources/base/profile/backup_config.json", import.meta.url,
    ), "utf8"));
    expect(config.allowToBackupRestore).toBe(false);
  });

  it("preserves application filesDir while keeping the framework initialization metadata", () => {
    const h = harness();
    expect(h.ability.createInitContext("rssh_lib")).toEqual({
      ...h.initContext, basePath: h.applicationContext.filesDir, prefPath: h.applicationContext.filesDir,
    });
    expect(h.createInitContext).toHaveBeenCalledExactlyOnceWith("rssh_lib");
    expect(h.ability.context.filesDir).toBe("/isolated-test/base/haps/entry/files");
  });

  it("lets NativeAbility own creation, teardown and cancellation", () => {
    const h = harness();
    for (const name of ["onCreate", "onWindowStageCreate", "onWindowStageDestroy", "onDestroy"] as const) {
      expect(h.ability[name]).toBe(h.NativeAbility.prototype[name]);
    }
  });

  it("assembles framework capabilities and creates independent application plugin instances", () => {
    const h = harness();
    expect(h.ability.moduleName).toBe("rssh_lib");
    expect(h.ability.bridgePlugins.map((factory) => factory.create().id).sort()).toEqual([
      "ohos.app-control", "ohos.files", "ohos.permission", "ohos.url", "ohos.webview", "ohos.window",
      "rssh.clipboard", "rssh.files-access", "rssh.runtime", "rssh.serial",
    ]);
    for (const factory of h.ability.bridgePlugins) expect(factory.create()).not.toBe(factory.create());
  });
});
