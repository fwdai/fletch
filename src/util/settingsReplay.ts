// Folding the setting writes that raced a settings read back over its answer.
//
// The host forwards events and writes op responses from separate tasks, so
// there is no order between them: a write made after the host served a read
// can reach the client BEFORE that read's answer does. Applying the answer
// wholesale then undoes the write, and the screen stays wrong until something
// reads again. So a reader buffers the changes that land while its read is in
// flight and replays them over the snapshot — an event is always newer than
// the snapshot it raced. The same rule as `replayApprovalEvents`
// (./publishApprovals), for the global settings (store/hostSettings) and one
// project's (./useProjectSettings).

import type { UnlistenFn } from "@tauri-apps/api/event";
import type { ProjectSettingsChangedEvent, SettingsChangedEvent } from "../api/types/settings";

/** `all` with one write applied; `null` deletes the row (back to the key's
 *  default). */
export function withSetting(
  all: Record<string, string>,
  key: string,
  value: string | null,
): Record<string, string> {
  const next = { ...all };
  if (value === null) delete next[key];
  else next[key] = value;
  return next;
}

/** The settings as they stand: what the read said, plus every write since, in
 *  the order they landed. */
export function replaySettingChanges(
  snapshot: Record<string, string>,
  changes: readonly SettingsChangedEvent[],
): Record<string, string> {
  return changes.reduce((all, c) => withSetting(all, c.key, c.value), snapshot);
}

/** Where one project's settings come from: the `get_project_settings` read and
 *  the `project_settings:changed` stream. Injected so the ordering below can be
 *  tested without a transport. */
export interface ProjectSettingsSource {
  read: (projectId: string) => Promise<Record<string, string>>;
  subscribe: (cb: (e: ProjectSettingsChangedEvent) => void) => Promise<UnlistenFn>;
}

type Fold = (prev: Record<string, string> | null) => Record<string, string>;

/** Read one project's settings and keep them live, handing every new value to
 *  `update`. Returns the stop: after it, nothing more is written — a read still
 *  in flight belongs to a load that was replaced (another project, a remount,
 *  an environment switch) and is dropped.
 *
 *  The subscription is made first and the read issued once it is live, so no
 *  write falls in the gap between them; events that land while the read is in
 *  flight are held and replayed over its answer. Nothing is written until the
 *  read lands, and nothing at all if it fails: the caller's null means "the
 *  host could not say". */
export function followProjectSettings(
  projectId: string,
  source: ProjectSettingsSource,
  update: (fold: Fold) => void,
): () => void {
  let alive = true;
  let loaded = false;
  let buffer: SettingsChangedEvent[] | null = [];

  const unlisten = source.subscribe((e) => {
    if (!alive || e.project_id !== projectId) return;
    if (loaded) update((prev) => withSetting(prev ?? {}, e.key, e.value));
    else buffer?.push({ key: e.key, value: e.value });
  });

  unlisten
    .then(() => (alive ? source.read(projectId) : null))
    .then((all) => {
      if (!alive || !all) return;
      const raced = buffer ?? [];
      buffer = null;
      loaded = true;
      update(() => replaySettingChanges(all, raced));
    })
    .catch((e) => {
      buffer = null;
      console.error("load project settings failed", e);
    });

  return () => {
    alive = false;
    void unlisten.then((off) => off()).catch(() => {});
  };
}
