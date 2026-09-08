export interface NavigatorPlatformInfo {
  userAgent: string;
  maxTouchPoints?: number;
}

export interface PlatformInfo {
  isIOS: boolean;
  isMobile: boolean;
}

/** Detect only the two mobile OS families this Tauri app ships. */
export function detectPlatform(navigatorInfo?: NavigatorPlatformInfo): PlatformInfo {
  if (!navigatorInfo) return { isIOS: false, isMobile: false };

  const { userAgent } = navigatorInfo;
  const isIPadDesktopUA = /Macintosh/i.test(userAgent)
    && (navigatorInfo.maxTouchPoints ?? 0) > 1;
  const isIOS = /iPhone|iPad|iPod/i.test(userAgent) || isIPadDesktopUA;
  // OpenHarmony covers HarmonyOS NEXT: ArkWeb UAs say "OpenHarmony" and
  // older builds say "HarmonyOS"; either way it rides the mobile UI.
  const isHarmony = /OpenHarmony|HarmonyOS/i.test(userAgent);
  return {
    isIOS,
    isMobile: isIOS || isHarmony || /Android/i.test(userAgent),
  };
}

const current = detectPlatform(
  typeof navigator === "undefined"
    ? undefined
    : { userAgent: navigator.userAgent, maxTouchPoints: navigator.maxTouchPoints },
);

// Separate from PlatformInfo: only flows that must diverge INSIDE mobile
// (e.g. SFTP pickers: SAF on Android, Downloads-dir staging on HarmonyOS)
// read this.
export const isHarmony =
  typeof navigator !== "undefined" && /OpenHarmony|HarmonyOS/i.test(navigator.userAgent);

export const isIOS = current.isIOS;
export const isMobile = current.isMobile;
