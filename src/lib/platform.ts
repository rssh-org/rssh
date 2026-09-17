interface NavigatorPlatformInfo {
  userAgent: string;
  maxTouchPoints?: number;
}

/** Platform-specific update/IME behavior only; never use this for layout. */
export function isIOSPlatform(navigatorInfo?: NavigatorPlatformInfo): boolean {
  if (!navigatorInfo) return false;
  const { userAgent, maxTouchPoints = 0 } = navigatorInfo;
  return /iPhone|iPad|iPod/i.test(userAgent)
    || (/Macintosh/i.test(userAgent) && maxTouchPoints > 1);
}

export const isIOS = isIOSPlatform(
  typeof navigator === "undefined" ? undefined : navigator,
);
