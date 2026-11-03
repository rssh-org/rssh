interface NavigatorPlatformInfo {
  userAgent: string;
  maxTouchPoints?: number;
}

/** Local browser IME behavior only; never use this for layout or host capabilities. */
export function isIOSPlatform(navigatorInfo?: NavigatorPlatformInfo): boolean {
  if (!navigatorInfo) return false;
  const { userAgent, maxTouchPoints = 0 } = navigatorInfo;
  return /iPhone|iPad|iPod/i.test(userAgent)
    || (/Macintosh/i.test(userAgent) && maxTouchPoints > 1);
}

/** Browser input policy; native apps report this through runtime capabilities.
 * Desktop-mode browsers may mask the OS; touch support alone cannot identify it. */
export function browserTerminalKeybar(navigatorInfo?: NavigatorPlatformInfo): boolean {
  if (!navigatorInfo) return false;
  const { userAgent } = navigatorInfo;
  return isIOSPlatform(navigatorInfo)
    || /Android/i.test(userAgent)
    || (/OpenHarmony|HarmonyOS/i.test(userAgent) && /\b(?:Phone|Tablet)\b/i.test(userAgent));
}

export const isIOS = isIOSPlatform(
  typeof navigator === "undefined" ? undefined : navigator,
);
