export interface NavigatorPlatformInfo {
  userAgent: string;
  maxTouchPoints?: number;
}

export interface PlatformInfo {
  isIOS: boolean;
  isHarmony: boolean;
  isMobile: boolean;
}

/** Detect the mobile runtimes this Tauri app ships. */
export function detectPlatform(navigatorInfo?: NavigatorPlatformInfo): PlatformInfo {
  if (!navigatorInfo) return { isIOS: false, isHarmony: false, isMobile: false };

  const { userAgent } = navigatorInfo;
  const isIPadDesktopUA = /Macintosh/i.test(userAgent)
    && (navigatorInfo.maxTouchPoints ?? 0) > 1;
  const isIOS = /iPhone|iPad|iPod/i.test(userAgent) || isIPadDesktopUA;
  const isAndroid = /Android/i.test(userAgent);
  // Android-compatible HarmonyOS installs still use the Android adapters.
  const isHarmony = !isAndroid && /OpenHarmony|HarmonyOS/i.test(userAgent);
  return {
    isIOS,
    isHarmony,
    isMobile: isIOS || isHarmony || isAndroid,
  };
}

const current = detectPlatform(
  typeof navigator === "undefined"
    ? undefined
    : { userAgent: navigator.userAgent, maxTouchPoints: navigator.maxTouchPoints },
);

export const isHarmony = current.isHarmony;
export const isIOS = current.isIOS;
export const isMobile = current.isMobile;
