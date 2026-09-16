import { useCallback, useEffect, useRef, useState } from "react";
import { ERROR_TTL_MS } from "../dictation/useDictation";
import { api } from "../store";
import { prepareFile } from "./prepare";
import { UploadAborted, uploadAttachment } from "./upload";

export interface StagedAttachment {
  /** Local key, for the chip. */
  id: string;
  /** The name it is (or will be) staged under on the host. */
  name: string;
  /** The staged path once the upload has landed; absent while uploading. */
  path?: string;
}

const newId = () =>
  globalThis.crypto?.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36)}`;

/** The files staged for one composer. Picking uploads straight away — the
 *  wait happens while the user is still typing, not at send — and each chip
 *  shows whether its file has landed. `paths` is what goes on the message;
 *  `uploading` holds the send until every chip has one. A failed upload drops
 *  its chip and reports in `error`, which clears on its own like dictation's. */
export function useAttachments() {
  const [items, setItems] = useState<StagedAttachment[]>([]);
  const [error, setError] = useState<string | null>(null);
  const errorTimer = useRef<number | null>(null);
  // One controller per upload in flight, so removing a chip mid-upload tells
  // the host to drop the partial file instead of finishing it for nobody.
  const inFlight = useRef(new Map<string, AbortController>());
  const live = useRef(true);

  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
      for (const c of inFlight.current.values()) c.abort();
      inFlight.current.clear();
      if (errorTimer.current !== null) window.clearTimeout(errorTimer.current);
    };
  }, []);

  const fail = useCallback((message: string) => {
    setError(message);
    if (errorTimer.current !== null) window.clearTimeout(errorTimer.current);
    errorTimer.current = window.setTimeout(() => {
      errorTimer.current = null;
      setError(null);
    }, ERROR_TTL_MS);
  }, []);

  const add = useCallback(
    (files: Iterable<File>) => {
      for (const file of files) {
        const id = newId();
        const controller = new AbortController();
        inFlight.current.set(id, controller);
        setItems((cur) => [...cur, { id, name: file.name || "attachment" }]);
        void (async () => {
          try {
            const prepared = await prepareFile(file);
            if (controller.signal.aborted) return;
            // The staged name can differ from the picked one (a re-encoded
            // photo becomes `.jpg`); the chip follows the file that was sent.
            setItems((cur) => cur.map((a) => (a.id === id ? { ...a, name: prepared.name } : a)));
            const path = await uploadAttachment(
              api,
              prepared.name,
              prepared.bytes,
              controller.signal,
            );
            if (!live.current) return;
            setItems((cur) => cur.map((a) => (a.id === id ? { ...a, path } : a)));
          } catch (e) {
            if (!live.current || e instanceof UploadAborted) return;
            setItems((cur) => cur.filter((a) => a.id !== id));
            fail(
              `Couldn't attach ${file.name || "file"}: ${e instanceof Error ? e.message : String(e)}`,
            );
          } finally {
            inFlight.current.delete(id);
          }
        })();
      }
    },
    [fail],
  );

  const remove = useCallback((id: string) => {
    inFlight.current.get(id)?.abort();
    inFlight.current.delete(id);
    setItems((cur) => cur.filter((a) => a.id !== id));
  }, []);

  /** After a send: the paths belong to the message now. Anything still
   *  uploading is dropped too — `uploading` should have held the send. */
  const clear = useCallback(() => {
    for (const c of inFlight.current.values()) c.abort();
    inFlight.current.clear();
    setItems([]);
  }, []);

  return {
    items,
    /** Staged paths, in the order they were picked, for the message. */
    paths: items.flatMap((a) => (a.path ? [a.path] : [])),
    uploading: items.some((a) => !a.path),
    error,
    add,
    remove,
    clear,
  };
}

export type Attachments = ReturnType<typeof useAttachments>;
