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

export const isIOS = isIOSPlatform(
  typeof navigator === "undefined" ? undefined : navigator,
);
