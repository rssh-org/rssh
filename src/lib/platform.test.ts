import { describe, expect, it } from "vitest";
import { isIOSPlatform } from "./platform.ts";

describe("iOS platform restrictions", () => {
  it.each([
    ["Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)", 5],
    ["Mozilla/5.0 (iPad; CPU OS 18_0 like Mac OS X)", 5],
    ["Mozilla/5.0 (iPod touch; CPU iPhone OS 15_7 like Mac OS X)", 5],
    ["Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15) AppleWebKit/605.1.15 Mobile/15E148 Safari/604.1", 5],
  ])("recognizes iOS including iPad desktop UA: %s", (userAgent, maxTouchPoints) => {
    expect(isIOSPlatform({ userAgent, maxTouchPoints })).toBe(true);
  });

  it.each([
    ["Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15)", 0],
    ["Mozilla/5.0 (Linux; Android 16)", 5],
    ["Mozilla/5.0 (Phone; OpenHarmony 5.0) ArkWeb/4.1.6.1 Mobile", 5],
    ["Mozilla/5.0 (PC; HarmonyOS 6.0) ArkWeb/5.0.0.0", 10],
    ["Mozilla/5.0 (X11; Linux x86_64)", 0],
  ])("does not apply iOS restrictions to another host: %s", (userAgent, maxTouchPoints) => {
    expect(isIOSPlatform({ userAgent, maxTouchPoints })).toBe(false);
  });

  it("works outside a browser", () => {
    expect(isIOSPlatform(undefined)).toBe(false);
  });
});
