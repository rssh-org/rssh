import { invoke } from "@tauri-apps/api/core";

/** One-stop migration helper for users coming from other tools. */
export const MIGRATE_URL = "https://rssh-org.github.io/migrate/";

export function openMigrate() {
  invoke("open_external_url", { url: MIGRATE_URL }).catch((e) =>
    console.error("open_external_url failed:", e),
  );
}
