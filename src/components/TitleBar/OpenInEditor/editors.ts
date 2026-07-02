import { api, type DetectedEditor } from "@/api";
import type { IconName } from "@/components/Icon";
import { EDITOR_LOGOS } from "./logos";

/** What a detected editor's tile shows: its brand logo, a glyph (terminal), or
 *  a monogram fallback. All render on the one premium tile surface, so there's
 *  no per-editor color here — this is glyph selection only. Labels come from
 *  the backend. */
export interface EditorFace {
  mono: string;
  icon?: IconName;
  /** Single-path brand glyph (viewBox 0 0 24 24), rendered white on the tile. */
  logo?: string;
}

const FACES: Record<string, EditorFace> = {
  cursor: { mono: "Cu" },
  vscode: { mono: "VS" },
  windsurf: { mono: "W" },
  zed: { mono: "Z" },
  sublime: { mono: "S" },
  terminal: { mono: "", icon: "terminal" },
};

/** Face for an editor id, with a monogram fallback for unknown ids so a
 *  newly-detected editor still renders a tile. */
export function editorFace(id: string): EditorFace {
  const base = FACES[id] ?? { mono: id.slice(0, 2).toUpperCase() };
  return { ...base, logo: EDITOR_LOGOS[id] };
}

// Detection scans PATH + the Applications folders — do it once per session and
// share the result across mounts (the launcher unmounts when no agent is open).
let cache: Promise<DetectedEditor[]> | null = null;

export function detectEditors(): Promise<DetectedEditor[]> {
  cache ??= api.detectEditors().catch(() => []);
  return cache;
}

export const EDITOR_PREF_KEY = "fletch:openInEditor";
