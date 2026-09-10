import type { UnlistenFn } from "@tauri-apps/api/event";
import { useCallback, useEffect, useState } from "react";
import {
  api,
  type DictationModelProgressEvent,
  type DictationModelStatus,
  onDictationModelProgress,
} from "@/api";

export interface DictationModelView {
  /** `null` until the first fetch resolves. */
  status: DictationModelStatus | null;
  /** The latest progress event, or `null` before one arrives (and after an
   *  action that makes it stale). Carries `model_id`, so a row must check it
   *  is the one being described. */
  progress: DictationModelProgressEvent | null;
  select: (id: string) => void;
  download: (id: string) => void;
  remove: (id: string) => void;
}

/** The model choice and each candidate's install state, kept live. The
 *  download runs in the backend and
 *  outlives this screen, so the state is fetched on mount (for a screen that
 *  opened mid-download, or after a failure whose event we never saw) and then
 *  followed through `dictation:model_progress`. Terminal events change what's
 *  on disk, so they trigger a re-fetch rather than a locally inferred status. */
export function useDictationModel(): DictationModelView {
  const [status, setStatus] = useState<DictationModelStatus | null>(null);
  const [progress, setProgress] = useState<DictationModelProgressEvent | null>(null);

  useEffect(() => {
    let disposed = false;
    const refresh = () => {
      void api
        .dictationModelStatus()
        .then((s) => {
          if (!disposed) setStatus(s);
        })
        .catch(() => {});
    };
    refresh();

    let unlisten: UnlistenFn | undefined;
    void onDictationModelProgress((e) => {
      if (disposed) return;
      setProgress(e);
      if (e.state === "installed" || e.state === "error") refresh();
    }).then((fn) => {
      // The effect may have been cleaned up while listen() was in flight.
      if (disposed) fn();
      else unlisten = fn;
    });

    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const act = useCallback((call: () => Promise<DictationModelStatus>) => {
    // Drop the last progress event: it describes the download this action
    // replaces (a failure being retried, or an install being removed).
    setProgress(null);
    void call()
      .then(setStatus)
      .catch(() => {});
  }, []);

  const select = useCallback((id: string) => act(() => api.setDictationModel(id)), [act]);
  // A download the user asked for on a row is also a choice of that row's
  // model, so it selects first — `dictation_model_download` only ever fetches
  // the selected one. Selecting what is already selected is a harmless
  // re-write, which keeps this one path for both a fresh download and a retry.
  const download = useCallback(
    (id: string) =>
      act(async () => {
        await api.setDictationModel(id);
        return api.dictationModelDownload();
      }),
    [act],
  );
  const remove = useCallback((id: string) => act(() => api.dictationModelRemove(id)), [act]);

  return { status, progress, select, download, remove };
}
