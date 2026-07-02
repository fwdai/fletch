import { api, type DetectedEditor } from "@/api";
import type { IconName } from "@/components/Icon";
import { EDITOR_LOGOS } from "./logos";

/** What a tool's tile shows: its brand logo, a terminal glyph, or a monogram —
 *  all on the one premium tile surface, so there's no per-editor color here.
 *  This is glyph selection only; labels come from the backend. */
export interface EditorFace {
  mono?: string;
  icon?: IconName;
  /** Single-path brand glyph (viewBox 0 0 24 24), rendered white on the tile. */
  logo?: string;
}

/** Face for a detected tool: brand logo if we have one, else a terminal glyph
 *  for terminals, else a monogram from the label so any tool still renders. */
export function editorFace(editor: DetectedEditor): EditorFace {
  const logo = EDITOR_LOGOS[editor.id];
  if (logo) return { logo };
  if (editor.kind === "terminal") return { icon: "terminal" };
  return { mono: monogram(editor.label) };
}

/** Up-to-two-letter monogram from a label: initials of the first two words, or
 *  the first two letters of a single word ("Nova" → "NO"). */
function monogram(label: string): string {
  const words = label.split(/\s+/).filter(Boolean);
  const letters = words.length > 1 ? words.map((w) => w[0]).join("") : label;
  return letters.slice(0, 2).toUpperCase();
}

// Detection scans PATH + the Applications folders — do it once per session and
// share the result across mounts (the launcher unmounts when no agent is open).
let cache: Promise<DetectedEditor[]> | null = null;

export function detectEditors(): Promise<DetectedEditor[]> {
  cache ??= api.detectEditors().catch(() => []);
  return cache;
}

export const EDITOR_PREF_KEY = "fletch:openInEditor";
