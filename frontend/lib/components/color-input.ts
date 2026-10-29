export function normalizeHexColor(value: string): string | null {
  const hex = value.trim().replace(/^#/, "");
  if (!/^(?:[\da-f]{3}|[\da-f]{6})$/i.test(hex)) return null;
  const expanded = hex.length === 3
    ? [...hex].map((digit) => digit + digit).join("")
    : hex;
  return `#${expanded.toUpperCase()}`;
}

// Existing imported highlight rules may use other xterm color formats.
// Preserve an untouched value without accepting new invalid HEX drafts.
export function isColorInputValid(value: string, savedValue?: string): boolean {
  return normalizeHexColor(value) !== null || (!!savedValue && value === savedValue);
}
