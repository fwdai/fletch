// Unified-diff parser for the diff viewer. `get_file_diff` returns raw git
// output; this turns it into hunks of typed lines with both gutters.

export type DiffLineKind = "ctx" | "add" | "rem";

export interface DiffLine {
  kind: DiffLineKind;
  old: number | null;
  next: number | null;
  text: string;
}

export interface DiffHunk {
  header: string;
  lines: DiffLine[];
}

const HEADER = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

export function parseUnifiedDiff(diff: string): DiffHunk[] {
  const hunks: DiffHunk[] = [];
  let current: DiffHunk | null = null;
  let oldNo = 0;
  let newNo = 0;
  for (const raw of diff.split("\n")) {
    const match = HEADER.exec(raw);
    if (match) {
      current = { header: raw, lines: [] };
      hunks.push(current);
      oldNo = Number(match[1]);
      newNo = Number(match[2]);
      continue;
    }
    if (!current) continue; // file headers before the first hunk
    if (raw.startsWith("+")) {
      current.lines.push({ kind: "add", old: null, next: newNo, text: raw.slice(1) });
      newNo += 1;
    } else if (raw.startsWith("-")) {
      current.lines.push({ kind: "rem", old: oldNo, next: null, text: raw.slice(1) });
      oldNo += 1;
    } else if (raw.startsWith("\\")) {
      // "\ No newline at end of file" — metadata, not a line of the file.
    } else {
      current.lines.push({ kind: "ctx", old: oldNo, next: newNo, text: raw.slice(1) });
      oldNo += 1;
      newNo += 1;
    }
  }
  return hunks;
}

/** git's single-letter status → the badge letter the design uses. */
export const STATUS_LETTER: Record<string, string> = {
  modified: "M",
  added: "A",
  deleted: "D",
  renamed: "R",
  untracked: "U",
  conflicted: "C",
};
