import { invoke } from "@tauri-apps/api/core";

/** The location is an opaque host reference. Names come from host metadata,
 * never from interpreting a content URI or document identifier in the UI. */
export interface PickedLocation {
  location: string;
  name: string | null;
}

export interface LocalWalkEntry {
  rel_path: string;
  size: number;
  local_path: string;
}

export function pickFile(): Promise<PickedLocation | null> {
  return invoke("sftp_pick_open_path");
}

export function pickFiles(): Promise<PickedLocation[] | null> {
  return invoke("sftp_pick_open_files");
}

export function pickDirectory(write: boolean): Promise<PickedLocation | null> {
  return invoke("sftp_pick_folder", { write });
}

/** A folder grant stays in the host. Its handle can accompany retries while
 * the selected location remains suitable for display in the transfer queue. */
export type SaveDestination = string | { location: string; grantId: number };

export function pickSavePath(defaultName: string): Promise<SaveDestination | null> {
  return invoke("sftp_pick_save_path", { defaultName });
}

export function walkDirectory(localRoot: string): Promise<LocalWalkEntry[]> {
  return invoke("walk_local_dir", { localRoot });
}

export type ResolvedPath =
  | { status: "ready"; relative_path: string; location: string }
  | { status: "failed"; relative_path: string; error: string };

export function resolvePaths(localRoot: string, relativePaths: string[], write: boolean): Promise<ResolvedPath[]> {
  return invoke("resolve_local_paths", { localRoot, relativePaths, write });
}
