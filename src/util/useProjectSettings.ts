import { useCallback, useEffect, useState } from "react";
import { api, onProjectSettingsChanged } from "@/api";
import { useAppStore } from "@/store";

/** One project's host-owned settings (docs/remote-protocol.md, "Settings"),
 *  read from the engine the UI is driving and kept live by
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
  const [settings, setSettings] = useState<Record<string, string> | null>(null);
  // Re-read (and re-subscribe) on a switch: both are bound to the transport
  // that was active when they were made.
  const environmentId = useAppStore((s) => s.activeEnvironmentId);

  // biome-ignore lint/correctness/useExhaustiveDependencies: environmentId is the intended re-run trigger, not an unused dep
  useEffect(() => {
    setSettings(null);
    if (!projectId) return;
    let alive = true;
    api
      .getProjectSettings(projectId)
      .then((all) => {
        if (alive) setSettings(all);
      })
      .catch((e) => console.error("load project settings failed", e));
    const unlisten = onProjectSettingsChanged((e) => {
      if (!alive || e.project_id !== projectId) return;
      setSettings((prev) => withValue(prev ?? {}, e.key, e.value));
    });
    return () => {
      alive = false;
      void unlisten.then((off) => off()).catch(() => {});
    };
  }, [projectId, environmentId]);

  const save = useCallback(
    (key: string, value: string | null) => {
      if (!projectId) return;
      setSettings((prev) => withValue(prev ?? {}, key, value));
      api
        .setProjectSetting(projectId, key, value)
        .catch((e) => console.error(`save ${key} failed`, e));
    },
    [projectId],
  );

  return { settings, save };
}

function withValue(
  all: Record<string, string>,
  key: string,
  value: string | null,
): Record<string, string> {
  const next = { ...all };
  if (value === null) delete next[key];
  else next[key] = value;
  return next;
}
