import { describe, expect, it } from "vitest";

import { detectPlatform } from "./platform.ts";

describe("detectPlatform", () => {
  it.each([
    ["iPhone", "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)", 5],
    ["iPad", "Mozilla/5.0 (iPad; CPU OS 18_0 like Mac OS X)", 5],
    ["iPod", "Mozilla/5.0 (iPod touch; CPU iPhone OS 15_7 like Mac OS X)", 5],
    [
      "iPadOS desktop UA",
      "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15) AppleWebKit/605.1.15 Version/18.0 Mobile/15E148 Safari/604.1",
      5,
    ],
  ])("recognizes %s as iOS mobile", (_name, userAgent, maxTouchPoints) => {
    expect(detectPlatform({ userAgent, maxTouchPoints })).toEqual({
      isIOS: true,
      isHarmony: false,
      isMobile: true,
    });
  });

  it("recognizes Android as mobile but not iOS", () => {
    expect(detectPlatform({ userAgent: "Mozilla/5.0 (Linux; Android 16)", maxTouchPoints: 5 }))
      .toEqual({ isIOS: false, isHarmony: false, isMobile: true });
  });

  it("recognizes HarmonyOS ArkWeb as mobile but not iOS", () => {
    expect(
      detectPlatform({
        userAgent:
          "Mozilla/5.0 (Phone; OpenHarmony 5.0) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/114.0.0.0 Safari/537.36 ArkWeb/4.1.6.1 Mobile",
        maxTouchPoints: 5,
      }),
    ).toEqual({ isIOS: false, isHarmony: true, isMobile: true });
  });

  it("recognizes HarmonyOS without an Android runtime", () => {
    expect(detectPlatform({ userAgent: "Mozilla/5.0 (Phone; HarmonyOS 5.0)" }))
      .toEqual({ isIOS: false, isHarmony: true, isMobile: true });
  });

  it.each(["OpenHarmony 5.1", "HarmonyOS 6.0"])("recognizes a touch-capable Harmony PC as desktop: %s", (system) => {
    expect(detectPlatform({
      userAgent: `Mozilla/5.0 (PC; ${system}) AppleWebKit/537.36 Chrome/132.0.0.0 Safari/537.36 ArkWeb/5.0.0.0`,
      maxTouchPoints: 10,
    })).toEqual({ isIOS: false, isHarmony: true, isMobile: false });
  });

  it("keeps Harmony tablets on the touch interface", () => {
    expect(detectPlatform({ userAgent: "Mozilla/5.0 (Tablet; OpenHarmony 5.1) ArkWeb/5.0.0.0" }))
      .toEqual({ isIOS: false, isHarmony: true, isMobile: true });
  });

  it("keeps Android-compatible HarmonyOS on the Android path", () => {
    expect(detectPlatform({ userAgent: "Mozilla/5.0 (Linux; Android 12; HarmonyOS 4.0)" }))
      .toEqual({ isIOS: false, isHarmony: false, isMobile: true });
  });

  it("recognizes a plain Linux desktop UA as desktop", () => {
    expect(detectPlatform({ userAgent: "Mozilla/5.0 (X11; Linux x86_64)" }))
      .toEqual({ isIOS: false, isHarmony: false, isMobile: false });
  });

  it("does not misclassify a touch-capable Mac or ordinary desktop", () => {
    expect(detectPlatform({ userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15)", maxTouchPoints: 1 }))
      .toEqual({ isIOS: false, isHarmony: false, isMobile: false });
    expect(detectPlatform({ userAgent: "Mozilla/5.0 (X11; Linux x86_64)", maxTouchPoints: 0 }))
      .toEqual({ isIOS: false, isHarmony: false, isMobile: false });
  });

  it("defaults safely outside a browser", () => {
    expect(detectPlatform(undefined)).toEqual({ isIOS: false, isHarmony: false, isMobile: false });
  });
});
