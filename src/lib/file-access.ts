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

export function pickSavePath(defaultName: string): Promise<string | null> {
  return invoke("sftp_pick_save_path", { defaultName });
}

export function walkDirectory(localRoot: string): Promise<LocalWalkEntry[]> {
  return invoke("walk_local_dir", { localRoot });
}

export function resolvePaths(localRoot: string, relativePaths: string[], write: boolean): Promise<string[]> {
  return invoke("resolve_local_paths", { localRoot, relativePaths, write });
}
