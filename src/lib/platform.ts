export interface NavigatorPlatformInfo {
  userAgent: string;
  maxTouchPoints?: number;
}

export interface PlatformInfo {
  isIOS: boolean;
  isHarmony: boolean;
  isMobile: boolean;
}

/** Device form factor only. Native features come from runtime capabilities. */
export function detectPlatform(navigatorInfo?: NavigatorPlatformInfo): PlatformInfo {
  if (!navigatorInfo) return { isIOS: false, isHarmony: false, isMobile: false };

  const { userAgent } = navigatorInfo;
  const isIPadDesktopUA = /Macintosh/i.test(userAgent)
    && (navigatorInfo.maxTouchPoints ?? 0) > 1;
  const isIOS = /iPhone|iPad|iPod/i.test(userAgent) || isIPadDesktopUA;
  const isAndroid = /Android/i.test(userAgent);
  // Android-compatible HarmonyOS installs still use the Android adapters.
  const isHarmony = !isAndroid && /OpenHarmony|HarmonyOS/i.test(userAgent);
  // ArkWeb uses Phone, Tablet and PC in the platform token. A Harmony PC
  // may have a touchscreen; touch points do not make it a mobile device.
  const isHarmonyPC = isHarmony && /\(PC(?:;|\))/i.test(userAgent);
  return {
    isIOS,
    isHarmony,
    isMobile: isIOS || (isHarmony && !isHarmonyPC) || isAndroid,
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
