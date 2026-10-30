import { useEffect, useMemo, useRef, useState } from "react";
import type { DirListing } from "@/api";
import type { AcPick, AcSource } from "@/components/Composer/autocomplete/types";
import {
  filterDirEntries,
  filterFiles,
  isFsPath,
  joinTypedDir,
  splitFsPath,
} from "@/components/Composer/mentions";

interface Args {
  query: string | null;
  /** Checkout-relative file paths for "@" search. Omit to disable. */
  mentionSource?: () => Promise<string[]>;
  /** Lists an arbitrary directory so "@~/…" completes real filesystem paths. */
  listDir?: (path: string) => Promise<DirListing>;
  /** Attach checkout-relative paths: the agent reads those in place. */
  addPaths: (paths: string[]) => void;
  /** Attach absolute paths picked off the filesystem listing, which may need
   *  staging first (see `stageEnvironmentPath`). Falls back to `addPaths`. */
  attachFsPaths?: (paths: string[]) => void;
  /** Attach the directory being listed itself — a folder the agent is given
   *  live, at its own path. Omit where there is no agent to give it to; the
   *  listing then only drills in. */
  attachFsDir?: (path: string) => void;
}

type Action =
  | { kind: "attach"; path: string }
  | { kind: "attachDir"; path: string }
  | { kind: "navigate"; query: string };

/** The "@" source: searches the agent's checkout files, or — when the query
 *  looks like a path (`~/…`, `/…`) — navigates the real filesystem. Picking a
 *  file attaches it; picking a directory rewrites the token to drill in. */
export function useFileSource({
  query,
  mentionSource,
  listDir,
  addPaths,
  attachFsPaths,
  attachFsDir,
}: Args): AcSource {
  const [files, setFiles] = useState<string[]>([]);
  // Cached listing for the directory being typed; `reqDir` guards against a
  // stale in-flight result for a different directory.
  const [fsListing, setFsListing] = useState<{
    reqDir: string;
    base: string;
    entries: DirListing["entries"];
  } | null>(null);

  const enabled = !!(mentionSource || listDir);
  const canAttachDir = !!attachFsDir;
  const active = enabled ? query : null;
  const fs = active !== null && listDir && isFsPath(active) ? splitFsPath(active) : null;

  const { rows, actions } = useMemo<{ rows: AcSource["rows"]; actions: Action[] }>(() => {
    if (active === null) return { rows: [], actions: [] };
    if (fs) {
      if (!fsListing || fsListing.reqDir !== fs.dir) return { rows: [], actions: [] };
      const base = fsListing.base;
      const matched = filterDirEntries(fsListing.entries, fs.partial);
      const rows: AcSource["rows"] = matched.map((e) => ({
        title: e.is_dir ? `${e.name}/` : e.name,
        icon: { file: e.name, folder: e.is_dir },
      }));
      const actions: Action[] = matched.map((e) =>
        e.is_dir
          ? { kind: "navigate", query: joinTypedDir(fs.dir, e.name) }
          : { kind: "attach", path: base.endsWith("/") ? base + e.name : `${base}/${e.name}` },
      );
      // Inside a directory (`@~/Projects/app/`, nothing typed after the
      // slash), the folder itself comes first: picking it gives the agent the
      // whole folder rather than one file.
      const dirRow = folderRow(base, fs.dir, fs.partial, canAttachDir);
      if (dirRow) {
        rows.unshift(dirRow);
        actions.unshift({ kind: "attachDir", path: base });
      }
      return { rows, actions };
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
  }, [active, fs?.dir, fs?.partial, fsListing, files, mentionSource, canAttachDir]);

  // Checkout mode: refetch the list each time the source opens (ref-held so
  // an inline `mentionSource` prop doesn't refire the effect).
  const checkoutActive = active !== null && !fs && !!mentionSource;
  const srcRef = useRef(mentionSource);
  srcRef.current = mentionSource;
  useEffect(() => {
    if (!checkoutActive || !srcRef.current) return;
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
  }, [checkoutActive]);

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
      (fs ? (attachFsPaths ?? addPaths) : addPaths)([a.path]);
      return { replace: "" };
    }
    if (a.kind === "attachDir") {
      attachFsDir?.(a.path);
      return { replace: "" };
    }
    // Drill into the directory: keep the "@" and the new path; caret lands at
    // the end so the next keystroke continues from inside it.
    return { replace: `@${a.query}` };
  };

  return { trigger: "@", heading: "Attach file", query: active, rows, pick };
}

/** The row that attaches the listed directory itself: offered only when
 *  folders can be attached and nothing is typed after the trailing slash (a
 *  partial name means the user is looking for something inside it), and never
 *  for `/`, a top-level folder like `/Users`, or home as typed (`@~/`). The
 *  rest of what a grant may not name (home by another path, hidden folders, …)
 *  is the backend's to refuse, with why. `typed` is the directory as the user
 *  typed it, `base` as the listing resolved it. */
export function folderRow(
  base: string,
  typed: string,
  partial: string,
  enabled: boolean,
): AcSource["rows"][number] | null {
  const dir = base.replace(/\/+$/, "");
  const home = typed.replace(/\/+$/, "") === "~";
  if (!enabled || partial !== "" || home || dir.split("/").length < 3) return null;
  const name = dir.slice(dir.lastIndexOf("/") + 1);
  return {
    title: `${name}/`,
    detail: "Give the agent this folder",
    icon: { glyph: "folder" },
  };
}
