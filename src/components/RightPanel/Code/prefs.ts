// The Code panel's two remembered choices, in one place so nothing else spells
// the keys: which mode it opens in, and what diffs measure against (the scope
// switch). Both are per machine, not per agent.
import type { DiffBaseMode } from "@/api";

export type CodeMode = "changes" | "explore";

const MODE_KEY = "q2:codeMode";
const BASE_KEY = "q2:diffBase";

/** Anything but "explore" opens Changes — including the retired "files" and
 *  "live" values earlier builds stored. */
export function parseMode(raw: string | null): CodeMode {
  return raw === "explore" ? "explore" : "changes";
}

export function parseBase(raw: string | null): DiffBaseMode {
  return raw === "head" ? "head" : "fork";
}

export const loadMode = (): CodeMode => parseMode(localStorage.getItem(MODE_KEY));
export const saveMode = (mode: CodeMode) => localStorage.setItem(MODE_KEY, mode);

export const loadBase = (): DiffBaseMode => parseBase(localStorage.getItem(BASE_KEY));
export const saveBase = (base: DiffBaseMode) => localStorage.setItem(BASE_KEY, base);

/** Make the Code panel open on every change since the fork. The panel reads
 *  these on mount, so call it before switching the right rail to Code. */
export function presetReviewAllChanges() {
  saveMode("changes");
  saveBase("fork");
}
