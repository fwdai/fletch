// Parse `git diff` unified output into hunks the Code/Live panel can render.
// Mirrors the prototype's hunk shape (fletch v2 data.jsx CODE_CHANGES):
//   { header, lines: [{ op, o, n, t }] }
// where `op` is context / addition / removal, `o`/`n` are the old/new 1-indexed
// line numbers (null on the side the line doesn't exist), and `t` is the line
// text without its leading sigil.

export type DiffOp = "ctx" | "add" | "rem";

export interface DiffLine {
  op: DiffOp;
  o: number | null;
  n: number | null;
  t: string;
}

export interface DiffHunk {
  header: string;
  lines: DiffLine[];
}

const HUNK_RE = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;

export function parseUnifiedDiff(text: string): DiffHunk[] {
  const hunks: DiffHunk[] = [];
  let hunk: DiffHunk | null = null;
  let oldLine = 0;
  let newLine = 0;

  for (const line of text.split("\n")) {
    const m = HUNK_RE.exec(line);
    if (m) {
      hunk = { header: line, lines: [] };
      hunks.push(hunk);
      oldLine = parseInt(m[1], 10);
      newLine = parseInt(m[2], 10);
      continue;
    }
    if (!hunk) continue; // skip the diff/index/---/+++ preamble
    if (line === "") continue; // trailing-newline artifact; real lines carry a sigil
    if (line.startsWith("\\")) continue; // "\ No newline at end of file"

    const sigil = line[0];
    const t = line.slice(1);
    if (sigil === "+") {
      hunk.lines.push({ op: "add", o: null, n: newLine, t });
      newLine++;
    } else if (sigil === "-") {
      hunk.lines.push({ op: "rem", o: oldLine, n: null, t });
      oldLine++;
    } else {
      // context line (leading space) — also covers a stray empty trailing line
      hunk.lines.push({ op: "ctx", o: oldLine, n: newLine, t });
      oldLine++;
      newLine++;
    }
  }

  return hunks;
}
