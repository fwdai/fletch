import { useCallback, useEffect, useState } from "react";
import { useEngine } from "@/api/engine";
import { followProjectSettings, withSetting } from "./settingsReplay";

/** One project's host-owned settings (docs/remote-protocol.md, "Settings"),
 *  read from the host the caller is scoped to and kept live by
 *  `project_settings:changed` — so a second desktop, or a phone, editing the
 *  same project moves this page without a refetch.
 *
 *  `settings` is null until the first read lands (and stays null when the host
 *  cannot answer it); an absent key is the key's default, which each caller
 *  spells. `save` writes optimistically and deletes the row for `null`. */
export function useProjectSettings(projectId: string | null | undefined): {
  settings: Record<string, string> | null;
  save: (key: string, value: string | null) => void;
} {
  const { api, events } = useEngine();
  const [settings, setSettings] = useState<Record<string, string> | null>(null);

  // `api` and `events` are the scope's host's: another host is a re-read.
  useEffect(() => {
    setSettings(null);
    if (!projectId) return;
    return followProjectSettings(
      projectId,
      { read: api.getProjectSettings, subscribe: events.onProjectSettingsChanged },
      setSettings,
    );
  }, [projectId, api, events]);

  const save = useCallback(
    (key: string, value: string | null) => {
      if (!projectId) return;
      setSettings((prev) => withSetting(prev ?? {}, key, value));
      api
        .setProjectSetting(projectId, key, value)
        .catch((e) => console.error(`save ${key} failed`, e));
    },
    [projectId, api],
  );

  return { settings, save };
}
