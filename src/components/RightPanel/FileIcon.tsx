// VS Code-style file/folder icons via the Material Icon Theme (MIT) — the
// same family Cursor/VS Code ship. The theme's precompiled manifest maps
// file names / extensions / folder names to icon ids.
//
// The SVGs are served as static assets from /file-icons (copied into public/
// by a Vite plugin, see vite.config.ts). The manifest itself is ~450 KB, so
// it's loaded lazily — the first FileIcon mount kicks off a dynamic import
// (splitting it out of the startup bundle); every icon shares that one promise
// and re-renders via `useSyncExternalStore` once it resolves.
import { useSyncExternalStore } from "react";

interface Manifest {
  iconDefinitions: Record<string, { iconPath: string }>;
  fileNames: Record<string, string>;
  fileExtensions: Record<string, string>;
  folderNames: Record<string, string>;
  folderNamesExpanded: Record<string, string>;
  file: string;
  folder: string;
  folderExpanded: string;
}

let manifest: Manifest | null = null;
let loading: Promise<void> | null = null;
const listeners = new Set<() => void>();

function loadManifest(): Promise<void> {
  loading ??= import("material-icon-theme/dist/material-icons.json")
    .then((mod) => {
      manifest = mod.default as unknown as Manifest;
      for (const notify of listeners) notify();
    })
    .catch((err) => {
      // Drop the rejected promise so a later mount can retry, rather than
      // caching the failure and leaving every icon a placeholder for good.
      loading = null;
      console.error("FileIcon: failed to load icon manifest", err);
    });
  return loading;
}

function useManifest(): Manifest | null {
  return useSyncExternalStore(
    (notify) => {
      listeners.add(notify);
      void loadManifest();
      return () => listeners.delete(notify);
    },
    () => manifest,
  );
}

/** icon id → svg basename, e.g. "typescript" → "typescript.svg"
 *  (most match by name, but ~72 point at a differently-named file). */
function basenameForId(m: Manifest, id: string): string {
  return m.iconDefinitions[id]?.iconPath.split("/").pop() ?? `${id}.svg`;
}

function fileBasename(m: Manifest, name: string): string {
  const lower = name.toLowerCase();
  let id: string | undefined = m.fileNames[lower];
  if (!id) {
    const parts = lower.split(".");
    // try the longest dotted suffix first: "a.test.ts" → "test.ts", then "ts"
    for (let i = 1; i < parts.length; i++) {
      const ext = parts.slice(i).join(".");
      if (m.fileExtensions[ext]) {
        id = m.fileExtensions[ext];
        break;
      }
    }
  }
  return basenameForId(m, id || m.file);
}

function folderBasename(m: Manifest, name: string, open: boolean): string {
  const named = (open ? m.folderNamesExpanded : m.folderNames)[name.toLowerCase()];
  return basenameForId(m, named || (open ? m.folderExpanded : m.folder));
}

interface FileIconProps {
  name: string;
  folder?: boolean;
  open?: boolean;
}

/** Material file/folder icon for the File panel. */
export function FileIcon({ name, folder, open }: FileIconProps) {
  const m = useManifest();
  // No src while the manifest loads: reserves the icon's box without a
  // broken-image glyph or a stray request.
  if (!m) return <img className="fp-ico-img" alt="" draggable={false} />;
  const base = folder ? folderBasename(m, name, !!open) : fileBasename(m, name);
  return <img className="fp-ico-img" src={`/file-icons/${base}`} alt="" draggable={false} />;
}
