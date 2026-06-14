import { useEffect, useMemo, useRef, useState } from "react";
import type { DirListing } from "../../../../api";
import {
  filterDirEntries,
  filterFiles,
  isFsPath,
  joinTypedDir,
  splitFsPath,
} from "../../mentions";
import type { AcPick, AcSource } from "../types";

interface Args {
  query: string | null;
  /** Worktree-relative file paths for "@" search. Omit to disable. */
  mentionSource?: () => Promise<string[]>;
  /** Lists an arbitrary directory so "@~/…" completes real filesystem paths. */
  listDir?: (path: string) => Promise<DirListing>;
  addPaths: (paths: string[]) => void;
}

type Action = { kind: "attach"; path: string } | { kind: "navigate"; query: string };

/** The "@" source: searches the agent's worktree files, or — when the query
 *  looks like a path (`~/…`, `/…`) — navigates the real filesystem. Picking a
 *  file attaches it; picking a directory rewrites the token to drill in. */
export function useFileSource({ query, mentionSource, listDir, addPaths }: Args): AcSource {
  const [files, setFiles] = useState<string[]>([]);
  // Cached listing for the directory being typed; `reqDir` guards against a
  // stale in-flight result for a different directory.
  const [fsListing, setFsListing] = useState<{
    reqDir: string;
    base: string;
    entries: DirListing["entries"];
  } | null>(null);

  const enabled = !!(mentionSource || listDir);
  const active = enabled ? query : null;
  const fs = active !== null && listDir && isFsPath(active) ? splitFsPath(active) : null;

  const { rows, actions } = useMemo<{ rows: AcSource["rows"]; actions: Action[] }>(() => {
    if (active === null) return { rows: [], actions: [] };
    if (fs) {
      if (!fsListing || fsListing.reqDir !== fs.dir) return { rows: [], actions: [] };
      const base = fsListing.base;
      const matched = filterDirEntries(fsListing.entries, fs.partial);
      return {
        rows: matched.map((e) => ({
          title: e.is_dir ? `${e.name}/` : e.name,
          icon: { file: e.name, folder: e.is_dir },
        })),
        actions: matched.map((e) =>
          e.is_dir
            ? { kind: "navigate", query: joinTypedDir(fs.dir, e.name) }
            : { kind: "attach", path: base.endsWith("/") ? base + e.name : `${base}/${e.name}` },
        ),
      };
    }
    if (!mentionSource) return { rows: [], actions: [] };
    const matched = filterFiles(files, active);
    return {
      rows: matched.map((p) => {
        const i = p.lastIndexOf("/");
        const name = i === -1 ? p : p.slice(i + 1);
        return {
          title: name,
          detail: i === -1 ? undefined : p.slice(0, i + 1),
          detailRtl: true,
          icon: { file: name },
        };
      }),
      actions: matched.map((p) => ({ kind: "attach", path: p })),
    };
    // `active`/`fs` recreate each render; depend on their primitive fields.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [active, fs?.dir, fs?.partial, fsListing, files, mentionSource]);

  // Worktree mode: refetch the list each time the source opens (ref-held so
  // an inline `mentionSource` prop doesn't refire the effect).
  const worktreeActive = active !== null && !fs && !!mentionSource;
  const srcRef = useRef(mentionSource);
  srcRef.current = mentionSource;
  useEffect(() => {
    if (!worktreeActive || !srcRef.current) return;
    let alive = true;
    srcRef
      .current()
      .then((f) => {
        if (alive) setFiles(f);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [worktreeActive]);

  // Filesystem mode: re-list only when the typed directory changes.
  const fsDir = fs?.dir ?? null;
  const listDirRef = useRef(listDir);
  listDirRef.current = listDir;
  useEffect(() => {
    if (fsDir === null || !listDirRef.current) return;
    let alive = true;
    listDirRef
      .current(fsDir)
      .then((res) => {
        if (alive) setFsListing({ reqDir: fsDir, base: res.base, entries: res.entries });
      })
      .catch(() => {
        if (alive) setFsListing(null);
      });
    return () => {
      alive = false;
    };
  }, [fsDir]);

  const pick = (i: number): AcPick | null => {
    const a = actions[i];
    if (!a) return null;
    if (a.kind === "attach") {
      addPaths([a.path]);
      return { replace: "" };
    }
    // Drill into the directory: keep the "@" and the new path; caret lands at
    // the end so the next keystroke continues from inside it.
    return { replace: `@${a.query}` };
  };

  return { trigger: "@", heading: "Attach file", query: active, rows, pick };
}
