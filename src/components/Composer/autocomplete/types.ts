// Shared model for the composer's autocompletions. A "source" (files via
// "@", PRs via "#", slash commands via "/") plugs into one menu + one set of
// keyboard mechanics by producing `AcRow`s and a `pick` that says how to
// rewrite the trigger token. See `useAutocomplete`.
import type { IconName } from "../../Icon";

/** Left-hand icon for a row: a Material file icon (by file/folder name) or a
 *  built-in UI glyph. */
export type AcIcon = { file: string; folder?: boolean } | { glyph: IconName };

/** One menu row. `title` is the prominent monospace label; `detail` is muted
 *  secondary text (set `detailRtl` to ellipsize from the left, e.g. for long
 *  directory paths where the tail matters most). */
export interface AcRow {
  title: string;
  detail?: string;
  detailRtl?: boolean;
  icon?: AcIcon;
}

/** The trigger token's span in the text: `start` is the trigger char, `end`
 *  is one past the token (found by scanning forward from the caret). */
export interface AcSpan {
  start: number;
  end: number;
}

/** Result of picking a row: replace the trigger token (`span`) with `replace`
 *  and put the caret at `start + (caretOffset ?? replace.length)`. Side
 *  effects (attaching a file, running a local command) happen inside `pick`. */
export interface AcPick {
  replace: string;
  caretOffset?: number;
}

/** One completion provider, built by a source hook that owns its own data
 *  loading keyed on `query`. */
export interface AcSource {
  /** Trigger character: "@", "#", "/". */
  trigger: string;
  /** Section label shown at the top of the menu. */
  heading: string;
  /** When true the trigger only fires at the start of a line (slash
   *  commands), rather than anywhere after whitespace (mentions). */
  lineStart?: boolean;
  /** Text between the trigger and the caret, or null when this source's
   *  trigger isn't the one under the caret. */
  query: string | null;
  rows: AcRow[];
  pick: (index: number, span: AcSpan) => AcPick | null;
}
