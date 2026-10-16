import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Group } from "../stores/app.svelte.ts";
import { environmentEvent } from "./environment-events.ts";

/** Subscribe before reading so changes during initialization cannot be missed. */
export function watchEnvironmentGroup(
  profileId: string,
  readProfile: () => Promise<{ group_id: string | null }>,
  readGroups: () => Promise<Group[]>,
  changed: (group: Group | null) => void,
  failed: (error: unknown) => void,
): UnlistenFn {
  let stopped = false;
  let revision = 0;
  let profileOff: UnlistenFn | undefined;
  let groupOff: UnlistenFn | undefined;
  let subscribedGroup: string | null = null;

  async function refresh() {
    const current = ++revision;
    try {
      const profile = await readProfile();
      if (stopped || current !== revision) return;
      if (profile.group_id !== subscribedGroup) {
        const off = profile.group_id
          ? await listen(environmentEvent("group", profile.group_id), () => { void refresh(); })
          : undefined;
        if (stopped || current !== revision) { off?.(); return; }
        groupOff?.();
        groupOff = off;
        subscribedGroup = profile.group_id;
      }
      const groups = await readGroups();
      if (!stopped && current === revision) changed(groups.find(g => g.id === profile.group_id) ?? null);
    } catch (error) {
      if (!stopped && current === revision) { changed(null); failed(error); }
    }
  }

  void listen(environmentEvent("connection", profileId), () => { void refresh(); })
    .then(off => {
      if (stopped) { off(); return; }
      profileOff = off;
      void refresh();
    }).catch(error => { if (!stopped) { changed(null); failed(error); } });

  return () => { stopped = true; revision += 1; profileOff?.(); groupOff?.(); };
}
