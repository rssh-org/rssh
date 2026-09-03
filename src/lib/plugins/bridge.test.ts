import { describe, expect, it } from "vitest";
import {
  execResponseErr,
  execResponseOk,
  helloEvent,
  isPluginRequest,
  readThemeTokens,
  visibilityEvent,
  withThemeFragment,
  MAX_COMMAND_LENGTH,
  MAX_REPORTED_SIZE,
  THEME_TOKENS,
  type ExecResult,
} from "./bridge.ts";

const req = (patch: Record<string, unknown> = {}) => ({
  v: 1,
  id: "r1",
  cmd: "exec",
  payload: { command: "cat /proc/stat", timeoutMs: 3000 },
  ...patch,
});

describe("isPluginRequest", () => {
  it("accepts a well-formed exec request", () => {
    expect(isPluginRequest(req())).toBe(true);
  });

  it("accepts exec without timeoutMs", () => {
    expect(isPluginRequest({v: 1, id: "r1", cmd: "exec", payload: {command: "uptime"}})).toBe(true);
  });

  it("rejects wrong protocol version", () => {
    expect(isPluginRequest(req({v: 2}))).toBe(false);
  });

  it("rejects unknown commands", () => {
    expect(isPluginRequest(req({cmd: "read_file"}))).toBe(false);
  });

  it("rejects missing or oversized ids", () => {
    expect(isPluginRequest(req({id: ""}))).toBe(false);
    expect(isPluginRequest(req({id: "x".repeat(65)}))).toBe(false);
  });

  it("rejects empty and over-cap commands", () => {
    expect(isPluginRequest(req({payload: {command: ""}}))).toBe(false);
    expect(
      isPluginRequest(req({payload: {command: "a".repeat(MAX_COMMAND_LENGTH + 1)}})),
    ).toBe(false);
  });

  it("rejects non-string timeoutMs", () => {
    expect(isPluginRequest(req({payload: {command: "ls", timeoutMs: "3000"}}))).toBe(false);
  });

  it("rejects non-finite, fractional and non-positive timeoutMs", () => {
    for (const timeoutMs of [Number.NaN, Number.POSITIVE_INFINITY, 0, -3000, null, 3000.5])
      expect(isPluginRequest(req({payload: {command: "ls", timeoutMs}}))).toBe(false);
    expect(isPluginRequest(req({payload: {command: "ls", timeoutMs: 3000}}))).toBe(true);
  });

  it("rejects non-object payloads", () => {
    expect(isPluginRequest(req({payload: null}))).toBe(false);
    expect(isPluginRequest("exec")).toBe(false);
  });
});

describe("size notifications", () => {
  const sizeReq = (payload: unknown) => ({v: 1, id: "s1", cmd: "size", payload});

  it("accepts width-only, height-only and both", () => {
    expect(isPluginRequest(sizeReq({height: 240}))).toBe(true);
    expect(isPluginRequest(sizeReq({width: 512}))).toBe(true);
    expect(isPluginRequest(sizeReq({width: 512, height: 240}))).toBe(true);
  });

  it("rejects empty and dimensionless payloads", () => {
    expect(isPluginRequest(sizeReq({}))).toBe(false);
    expect(isPluginRequest(sizeReq(null))).toBe(false);
  });

  it("rejects non-finite, non-positive and over-cap dimensions", () => {
    expect(isPluginRequest(sizeReq({height: 0}))).toBe(false);
    expect(isPluginRequest(sizeReq({height: -5}))).toBe(false);
    expect(isPluginRequest(sizeReq({height: Number.POSITIVE_INFINITY}))).toBe(false);
    expect(isPluginRequest(sizeReq({height: "tall"}))).toBe(false);
    expect(isPluginRequest(sizeReq({height: MAX_REPORTED_SIZE + 1}))).toBe(false);
    // A sane dimension next to an insane one still invalidates the frame.
    expect(isPluginRequest(sizeReq({width: 100, height: MAX_REPORTED_SIZE + 1}))).toBe(false);
  });
});

describe("host frames", () => {
  const result: ExecResult = {stdout: "ok", stderr: "", exitCode: 0};

  it("ok response carries the result verbatim", () => {
    expect(execResponseOk("r1", result)).toEqual({
      v: 1, id: "r1", ok: true, result,
    });
  });

  it("error response carries a code for the plugin to branch on", () => {
    expect(execResponseErr("r1", "plugin_no_exec", "no session")).toEqual({
      v: 1, id: "r1", ok: false, error: {code: "plugin_no_exec", message: "no session"},
    });
  });

  it("hello carries the theme tokens, visibility is a v1 frame", () => {
    expect(helloEvent({"--bg": "#2B2D3A"})).toEqual({
      v: 1,
      evt: "hello",
      payload: {apiVersion: 1, theme: {"--bg": "#2B2D3A"}},
    });
    expect(helloEvent().payload.theme).toBeUndefined();
    expect(visibilityEvent(false)).toEqual({v: 1, evt: "visibility", payload: {visible: false}});
  });
});

describe("theme tokens", () => {
  it("reads only known non-empty tokens off a computed style", () => {
    const style = {
      getPropertyValue: (name: string) =>
        name === "--bg" ? " #2B2D3A " : name === "--text" ? "" : `value-of(${name})`,
    };
    const tokens = readThemeTokens(style);
    expect(tokens["--bg"]).toBe("#2B2D3A"); // trimmed
    expect("--text" in tokens).toBe(false); // empty dropped
    expect(Object.keys(tokens)).toHaveLength(THEME_TOKENS.length - 1);
  });

  it("withThemeFragment appends an encoded fragment and drops any old one", () => {
    const url = withThemeFragment("asset://localhost/p/preview.html#old", {"--bg": "#2B2D3A"});
    expect(url.startsWith("asset://localhost/p/preview.html#rssh-theme=")).toBe(true);
    expect(url).not.toContain("#old");
    // Round-trip: the preview side JSON-parses the decoded fragment.
    const encoded = url.split("#rssh-theme=")[1];
    expect(JSON.parse(decodeURIComponent(encoded))).toEqual({"--bg": "#2B2D3A"});
  });
});
