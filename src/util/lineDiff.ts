// Line-level diff counts, matching git's "+X -Y" shortstat semantics.

export interface LineDiffCounts {
  additions: number;
  deletions: number;
}

/** Split text into counted lines. A trailing newline does not add a phantom
 *  empty line, so "a\n" => ["a"] (matches how git counts content lines). */
function toLines(text: string): string[] {
  if (text === "") return [];
  const lines = text.split("\n");
  if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
  return lines;
}

// Above this many DP cells, fall back to an order-insensitive multiset diff to
// avoid an O(m*n) table on pathologically large inputs.
const MAX_DP_CELLS = 2_000_000;

/** Order-insensitive approximation: common = sum of min occurrences per line. */
function multisetCounts(a: string[], b: string[]): LineDiffCounts {
  const counts = new Map<string, number>();
  for (const line of a) counts.set(line, (counts.get(line) ?? 0) + 1);
  let common = 0;
  for (const line of b) {
    const c = counts.get(line) ?? 0;
    if (c > 0) {
      common++;
      counts.set(line, c - 1);
    }
  }
  return { additions: b.length - common, deletions: a.length - common };
}

/** dp[i][j] = length of the LCS of a[i..] and b[j..]. */
function lcsTable(a: string[], b: string[]): number[][] {
  const m = a.length;
  const n = b.length;
  const dp = Array.from({ length: m + 1 }, () => new Array<number>(n + 1).fill(0));
  for (let i = m - 1; i >= 0; i--) {
    for (let j = n - 1; j >= 0; j--) {
      dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }
  return dp;
}

/** Added/removed line counts between two texts, using an LCS so unchanged
 *  lines aren't counted (e.g. replacing 9 lines with 23 where 5 match yields
 *  +18 -4, not +23 -9). */
export function lineDiffCounts(oldText: string, newText: string): LineDiffCounts {
  const a = toLines(oldText);
  const b = toLines(newText);
  if (a.length === 0) return { additions: b.length, deletions: 0 };
  if (b.length === 0) return { additions: 0, deletions: a.length };

  const m = a.length;
  const n = b.length;
  if (m * n > MAX_DP_CELLS) return multisetCounts(a, b);

  const lcs = lcsTable(a, b)[0][0];
  return { additions: n - lcs, deletions: m - lcs };
}

/** One line of a unified diff: kept, added or removed. */
export interface DiffLine {
  op: "keep" | "add" | "del";
  text: string;
}

/** The lines of `oldText` → `newText` as a unified diff, by the same LCS as
 *  `lineDiffCounts`. Past the DP budget it gives up on alignment and shows
 *  every old line removed, then every new line added — still the truth, just
 *  not the tightest one. */
export function lineDiffLines(oldText: string, newText: string): DiffLine[] {
  const a = toLines(oldText);
  const b = toLines(newText);
  const whole = (): DiffLine[] => [
    ...a.map((text): DiffLine => ({ op: "del", text })),
    ...b.map((text): DiffLine => ({ op: "add", text })),
  ];
  if (a.length === 0 || b.length === 0 || a.length * b.length > MAX_DP_CELLS) return whole();

  const dp = lcsTable(a, b);
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      out.push({ op: "keep", text: a[i] });
      i++;
      j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      out.push({ op: "del", text: a[i++] });
    } else {
      out.push({ op: "add", text: b[j++] });
    }
  }
  while (i < a.length) out.push({ op: "del", text: a[i++] });
  while (j < b.length) out.push({ op: "add", text: b[j++] });
  return out;
}
