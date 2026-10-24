import type { ConnectionKind } from "../stores/app.svelte.ts";

const CONNECTION_KINDS: ConnectionKind[] = ["ssh", "forward", "serial", "telnet"];

export function connectionCopyName(sourceName: string): string {
  return `${sourceName}_copy`;
}

export function availableConnectionKinds(serialSupported: boolean): ConnectionKind[] {
  return CONNECTION_KINDS.filter((kind) => serialSupported || kind !== "serial");
}
