import { describe, it, expect } from "vitest";
import { commandOutput, truncateCommand, formatBytes } from "./format.ts";

describe("truncateCommand", () => {
  it("leaves short commands untouched", () => {
    expect(truncateCommand("ls -l")).toBe("ls -l");
  });
  it("ellipsizes when over the cap", () => {
    const long = "a".repeat(200);
    const out = truncateCommand(long);
    expect(out.length).toBe(121); // 120 + "…"
    expect(out.endsWith("…")).toBe(true);
  });
  it("honors a custom cap", () => {
    expect(truncateCommand("123456789", 4)).toBe("1234…");
  });
});

describe("formatBytes", () => {
  it("keeps bytes integer", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(812)).toBe("812 B");
  });
  it("scales to KB/MB/GB with one decimal", () => {
    expect(formatBytes(1024)).toBe("1.0 KB");
    expect(formatBytes(52_428_800)).toBe("50.0 MB");
    expect(formatBytes(1_610_612_736)).toBe("1.5 GB");
  });
});

it("localizes a transport failure while preserving the same bytes as successful shell output", () => {
  const wire = '__rssh_err__|{"code":"session_not_found","params":{"id":"original-target"}}';
  expect(commandOutput(wire, -1)).not.toContain("__rssh_err__");
  expect(commandOutput(wire, 0)).toBe(wire);
  expect(commandOutput("partial shell output", -1)).toBe("partial shell output");
});
