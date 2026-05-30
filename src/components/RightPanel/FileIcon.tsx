// VS Code-style file/folder icons via the Material Icon Theme (MIT) — the
// same family Cursor/VS Code ship. The theme's precompiled manifest maps
// file names / extensions / folder names to icon ids.
//
// The SVGs are served as static assets from /file-icons (copied into public/
// by a Vite plugin, see vite.config.ts), so resolution is synchronous and the
// 1200+ icon set adds no per-icon chunks to the bundle.
import manifest from "material-icon-theme/dist/material-icons.json";

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
const m = manifest as unknown as Manifest;

/** icon id → svg basename, e.g. "typescript" → "typescript.svg"
 *  (most match by name, but ~72 point at a differently-named file). */
function basenameForId(id: string): string {
  return m.iconDefinitions[id]?.iconPath.split("/").pop() ?? `${id}.svg`;
}

function fileBasename(name: string): string {
  const lower = name.toLowerCase();
  let id: string | undefined = m.fileNames[lower];
  if (!id) {
    const parts = lower.split(".");
    // try the longest dotted suffix first: "a.test.ts" → "test.ts", then "ts"
    for (let i = 1; i < parts.length; i++) {
      const ext = parts.slice(i).join(".");
      if (m.fileExtensions[ext]) { id = m.fileExtensions[ext]; break; }
    }
  }
  return basenameForId(id || m.file);
}

function folderBasename(name: string, open: boolean): string {
  const named = (open ? m.folderNamesExpanded : m.folderNames)[name.toLowerCase()];
  return basenameForId(named || (open ? m.folderExpanded : m.folder));
}

interface FileIconProps {
  name: string;
  folder?: boolean;
  open?: boolean;
}

/** Material file/folder icon for the File panel. */
export function FileIcon({ name, folder, open }: FileIconProps) {
  const base = folder ? folderBasename(name, !!open) : fileBasename(name);
  return <img className="fp-ico-img" src={`/file-icons/${base}`} alt="" draggable={false} />;
}
