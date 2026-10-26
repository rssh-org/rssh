import * as fs from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import { afterEach, describe, expect, it, vi } from "vitest";

const source = fileURLToPath(new URL("../app/hvigorfile.ts", import.meta.url));
const code = ts.transpileModule(fs.readFileSync(source, "utf8"), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
}).outputText;
const roots: string[] = [];
const signingConfigs = [{ name: "default", type: "HarmonyOS", material: {
  storeFile: "certs/release.p12", storePassword: "test-store-secret", keyAlias: "release",
  keyPassword: "test-key-secret", signAlg: "SHA256withECDSA", profile: "certs/release.p7b", certpath: "certs/release.cer",
} }];
const validLocal = JSON.stringify({ signingConfigs });
afterEach(() => roots.splice(0).forEach((root) => fs.rmSync(root, { recursive: true, force: true })));

function harness(local?: string, signTask: string | null = "true", embeddedSigning = false) {
  const root = fs.mkdtempSync(path.join(tmpdir(), "rssh signing "));
  roots.push(root);
  const profile = {
    app: { signingConfigs: embeddedSigning ? [{ name: "embedded" }] : [],
      products: [{ name: "default", signingConfig: "default", compatibleSdkVersion: "5.0.0(12)" }] },
    modules: [{ name: "entry", srcPath: "./products/phone" }, { name: "desktop", srcPath: "./products/desktop" }],
  };
  const profileFile = path.join(root, "build-profile.json5");
  const publicContents = JSON.stringify(profile);
  fs.writeFileSync(profileFile, publicContents);
  const localFile = path.join(root, "signing.local.json");
  if (local !== undefined) fs.writeFileSync(localFile, local);
  const setBuildProfileOpt = vi.fn();
  const context = { getBuildProfileOpt: () => structuredClone(profile), setBuildProfileOpt };
  let evaluate: (node: unknown) => void = () => { throw new Error("Missing configuration hook"); };
  const node = { getNodePath: () => root, getContext: () => context,
    afterNodeEvaluate: (fn: typeof evaluate) => { evaluate = fn; } };
  const modules: Record<string, unknown> = {
    "node:fs": fs, "node:path": path,
    "@ohos/hvigor": { hvigor: { getRootNode: () => node,
      getParameter: () => ({ getExtParam: () => signTask ?? undefined }) } },
    "@ohos/hvigor-ohos-plugin": { appTasks: {}, OhosPluginId: { OHOS_APP_PLUGIN: "com.ohos.app" } },
  };
  runInNewContext(code, { exports: {}, require: (name: string) => {
    if (!(name in modules)) throw new Error(`Unexpected dependency: ${name}`);
    return modules[name];
  } }, { filename: source });
  return { evaluate: () => evaluate(node), setBuildProfileOpt, profile, profileFile, publicContents, localFile };
}

describe("HarmonyOS local signing configuration", () => {
  it("injects only signing materials before module evaluation without rewriting either file", () => {
    const h = harness(validLocal);
    expect(h.setBuildProfileOpt).not.toHaveBeenCalled();
    h.evaluate();
    expect(h.setBuildProfileOpt).toHaveBeenCalledWith({ ...h.profile, app: { ...h.profile.app, signingConfigs } });
    expect(fs.readFileSync(h.profileFile, "utf8")).toBe(h.publicContents);
    expect(fs.readFileSync(h.localFile, "utf8")).toBe(validLocal);
  });

  it("loads local signing for IDE builds without an explicit sign-task flag", () => {
    const h = harness(validLocal, null);
    h.evaluate();
    expect(h.setBuildProfileOpt).toHaveBeenCalledWith({ ...h.profile, app: { ...h.profile.app, signingConfigs } });
  });

  it("allows IDE configuration without signing materials", () => {
    const h = harness(undefined, null);
    h.evaluate();
    expect(h.setBuildProfileOpt).not.toHaveBeenCalled();
  });

  it("allows unsigned builds without local credentials", () => {
    const h = harness(undefined, "false");
    h.evaluate();
    expect(h.setBuildProfileOpt).not.toHaveBeenCalled();
  });

  it("does not parse local credentials when signing is explicitly disabled", () => {
    const h = harness("invalid local credentials", "false");
    h.evaluate();
    expect(h.setBuildProfileOpt).not.toHaveBeenCalled();
  });

  it("fails an explicitly signed build when local credentials are missing", () => {
    expect(() => harness().evaluate()).toThrow(/signing\.local\.json/);
  });

  it.each(['{"signingConfigs": ["test-private-password"', '{"signingConfigs": "test-private-password"}', '{"signingConfigs": []}'])
    ("rejects malformed local configuration without printing its contents", (local) => {
      const h = harness(local);
      let message = "";
      try { h.evaluate(); } catch (error) { message = String(error); }
      expect(message).toMatch(/signing\.local\.json/);
      expect(message).not.toContain("test-private-password");
      expect(h.setBuildProfileOpt).not.toHaveBeenCalled();
    });

  it("rejects signing materials reintroduced into the public build profile", () => {
    expect(() => harness(undefined, "false", true).evaluate()).toThrow(/build-profile\.json5/);
  });
});
