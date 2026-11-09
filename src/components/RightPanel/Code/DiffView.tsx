// Shared diff rendering: the Changes diff pane (Code panel and Git panel) and
// the file editor's "Diff" toggle all fetch a file's unified diff and render it
// here, so there's a single implementation of the diff markup + highlighting.
import { useEffect, useMemo, useRef, useState } from "react";
import { api, type DiffBaseMode } from "@/api";
import { hljsLang } from "@/data/languages";
import { type DiffHunk, type DiffLine, parseUnifiedDiff } from "@/util/diff";
import { highlightToHtml } from "@/util/highlight";
import type { WalkthroughRange } from "./Changes/Walkthrough";

export const extOf = (path: string) => path.split(".").pop() ?? "";

/** Identity of an added line for fresh-line tracking: its new-side number and
 *  text, so a line that merely shifted down still counts as seen. */
export const freshKey = (l: DiffLine) => `${l.n}:${l.t}`;

/** How long a walkthrough's target range stays highlighted. */
const FOCUS_FLASH_MS = 1500;

export interface FileDiffState {
  hunks: DiffHunk[];
  /** The failure's actual message (null when healthy) — surfacing it is what
   *  turns "Couldn't load diff" from a dead end into a bug report. */
  error: string | null;
  /** Whether `hunks`/`error` describe the requested file and base. False from
   *  the moment either changes until its fetch settles, so a pane never shows
   *  the previous file's diff under the new file's header. */
  loaded: boolean;
}

/** Fetch + parse a file's unified diff vs `base` (fork point when omitted).
 *  `dep` lets the caller force a refetch (e.g. when the file's +/- counts move
 *  during a turn); a refetch keeps the current hunks until the new ones land. */
export function useFileDiff(
  agentId: string,
  path: string | null,
  dep?: string,
  base?: DiffBaseMode,
): FileDiffState {
  const key = path ? `${agentId}\n${base ?? ""}\n${path}` : null;
  const [state, setState] = useState<{ key: string; hunks: DiffHunk[]; error: string | null }>();

  // biome-ignore lint/correctness/useExhaustiveDependencies: `dep` is the caller's refetch trigger
  useEffect(() => {
    if (!path || !key) return;
    let cancelled = false;
    api
      .getFileDiff(agentId, path, base)
      .then((t) => {
        if (!cancelled) setState({ key, hunks: parseUnifiedDiff(t), error: null });
      })
      .catch((e) => {
        if (!cancelled) {
          setState({ key, hunks: [], error: e instanceof Error ? e.message : String(e) });
        }
      });
    return () => {
      cancelled = true;
    };
  }, [agentId, path, dep, base, key]);

  const current = state?.key === key ? state : null;
  return { hunks: current?.hunks ?? [], error: current?.error ?? null, loaded: current != null };
}

interface DiffBodyProps {
  hunks: DiffHunk[];
  lang: string;
  isBuiltInTheme: boolean;
  /** The agent is writing this file now: shows the shimmer. */
  live?: boolean;
  /** Just-arrived added lines (by `freshKey`), animated in. */
  freshKeys?: Set<string>;
  /** New-side lines to bring into view and highlight briefly. A new object is
   *  a new request, even for the same numbers. */
  focusRange?: WalkthroughRange;
}

/** Render parsed hunks. */
export function DiffBody({
  hunks,
  lang,
  isBuiltInTheme,
  live = false,
  freshKeys,
  focusRange,
}: DiffBodyProps) {
  const ref = useRef<HTMLDivElement>(null);
  const shown = useRef<WalkthroughRange | null>(null);
  const [flash, setFlash] = useState<WalkthroughRange | null>(null);

  // Land on a requested range once, as soon as there are rows to land on; a
  // live refetch of the same file must not yank the scroll back to it.
  useEffect(() => {
    if (!focusRange || focusRange === shown.current || hunks.length === 0) return;
    shown.current = focusRange;
    const rows = ref.current?.querySelectorAll<HTMLElement>("[data-n]") ?? [];
    const target = Array.from(rows).find((r) => Number(r.dataset.n) >= focusRange.start);
    target?.scrollIntoView({ block: "center" });
    setFlash(focusRange);
  }, [focusRange, hunks]);

  useEffect(() => {
    if (!flash) return;
    const t = setTimeout(() => setFlash(null), FOCUS_FLASH_MS);
    return () => clearTimeout(t);
  }, [flash]);

  return (
    <div
      ref={ref}
      className={`code-diff text-sm ${isBuiltInTheme ? "cq" : ""} ${live ? "live" : ""}`}
    >
      <div className="code-diff-inner">
        {hunks.map((h) => (
          <div key={h.header}>
            <div className="code-hunk-h text-xs">{h.header}</div>
            {h.lines.map((l) => (
              <DiffLineRow
                key={`${l.o}:${l.n}`}
                line={l}
                lang={lang}
                fresh={!!(freshKeys && l.op === "add" && freshKeys.has(freshKey(l)))}
                focused={!!flash && l.n != null && l.n >= flash.start && l.n <= flash.end}
              />
            ))}
          </div>
        ))}
      </div>
    </div>
  );
}

function DiffLineRow({
  line,
  lang,
  fresh,
  focused,
}: {
  line: DiffLine;
  lang: string;
  fresh: boolean;
  focused: boolean;
}) {
  const sigil = line.op === "add" ? "+" : line.op === "rem" ? "−" : " ";
  const html = useMemo(
    () => (line.t ? highlightToHtml(line.t, hljsLang(lang) ? lang : "") : ""),
    [line.t, lang],
  );
  return (
    <div
      className={`dl op-${line.op}${fresh ? " fresh" : ""}${focused ? " focus" : ""}`}
      data-n={line.n ?? undefined}
    >
      <span className="dl-num o text-xs">{line.o ?? ""}</span>
      <span className="dl-num n text-xs">{line.n ?? ""}</span>
      <span className="dl-sigil">{sigil}</span>
      <span className="dl-text" dangerouslySetInnerHTML={{ __html: html }} />
    </div>
  );
}

/** A fetched diff in whichever state it is in: loading, failed, empty, or
 *  rendered. */
export function DiffPane({
  diff,
  ...body
}: Omit<DiffBodyProps, "hunks"> & { diff: FileDiffState }) {
  if (!diff.loaded) {
    return (
      <div className="empty-msg" style={{ margin: "auto" }}>
        <div className="et">Loading diff…</div>
      </div>
    );
  }
  if (diff.error) {
    return (
      <div className="empty-msg" style={{ margin: "auto" }}>
        <div className="et">Couldn't load diff</div>
        <div>{diff.error}</div>
      </div>
    );
  }
  if (diff.hunks.length === 0) {
    return (
      <div className="empty-msg" style={{ margin: "auto" }}>
        <div className="et">No textual diff</div>
        <div>This change has no line-level diff to show.</div>
      </div>
    );
  }
  return <DiffBody hunks={diff.hunks} {...body} />;
}

/** Self-contained file diff for the editor's Diff toggle, where there's no
 *  follow/fresh behavior. */
export function FileDiff({
  agentId,
  path,
  lang,
  isBuiltInTheme,
  base,
}: {
  agentId: string;
  path: string;
  lang: string;
  isBuiltInTheme: boolean;
  base?: DiffBaseMode;
}) {
  const diff = useFileDiff(agentId, path, undefined, base);
  return <DiffPane diff={diff} lang={lang} isBuiltInTheme={isBuiltInTheme} />;
}
