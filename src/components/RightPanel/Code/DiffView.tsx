// Shared diff rendering for the Code panel: the Live feed and the file
// editor's "Diff" toggle both fetch a file's unified diff and render it here,
// so there's a single implementation of the diff markup + syntax highlighting.
import { useEffect, useMemo, useState } from "react";
import { api } from "@/api";
import { hljsLang } from "@/data/languages";
import { type DiffHunk, type DiffLine, parseUnifiedDiff } from "@/util/diff";
import { highlightToHtml } from "@/util/highlight";

export const extOf = (path: string) => path.split(".").pop() ?? "";

/** Fetch + parse a file's unified diff vs the parent branch. `dep` lets the
 *  caller force a refetch (e.g. when the file's +/- counts move during a turn). */
export function useFileDiff(agentId: string, path: string | null, dep?: string) {
  const [hunks, setHunks] = useState<DiffHunk[]>([]);
  const [error, setError] = useState(false);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    if (!path) {
      setHunks([]);
      setError(false);
      setLoaded(false);
      return;
    }
    let cancelled = false;
    api
      .getFileDiff(agentId, path)
      .then((t) => {
        if (!cancelled) {
          setHunks(parseUnifiedDiff(t));
          setError(false);
          setLoaded(true);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setHunks([]);
          setError(true);
          setLoaded(true);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [agentId, path, dep]);

  return { hunks, error, loaded };
}

/** Render parsed hunks. `live` shows the agent-is-writing shimmer; `freshKeys`
 *  marks just-arrived added lines (both used only by the Live feed). */
export function DiffBody({
  hunks,
  lang,
  isBuiltInTheme,
  live = false,
  freshKeys,
}: {
  hunks: DiffHunk[];
  lang: string;
  isBuiltInTheme: boolean;
  live?: boolean;
  freshKeys?: Set<string>;
}) {
  return (
    <div className={`code-diff text-sm ${isBuiltInTheme ? "cq" : ""} ${live ? "live" : ""}`}>
      {hunks.map((h, i) => (
        <div key={i}>
          <div className="code-hunk-h text-xs">{h.header}</div>
          {h.lines.map((l, j) => (
            <DiffLineRow
              key={j}
              line={l}
              lang={lang}
              fresh={!!(freshKeys && l.op === "add" && freshKeys.has(`${l.n}:${l.t}`))}
            />
          ))}
        </div>
      ))}
    </div>
  );
}

function DiffLineRow({ line, lang, fresh }: { line: DiffLine; lang: string; fresh: boolean }) {
  const sigil = line.op === "add" ? "+" : line.op === "rem" ? "−" : " ";
  const html = useMemo(
    () => (line.t ? highlightToHtml(line.t, hljsLang(lang) ? lang : "") : ""),
    [line.t, lang],
  );
  return (
    <div className={`dl op-${line.op}${fresh ? " fresh" : ""}`}>
      <span className="dl-num o text-xs">{line.o ?? ""}</span>
      <span className="dl-num n text-xs">{line.n ?? ""}</span>
      <span className="dl-sigil">{sigil}</span>
      <span className="dl-text" dangerouslySetInnerHTML={{ __html: html }} />
    </div>
  );
}

/** Self-contained file diff (fetch + states + body) for the editor's Diff
 *  toggle, where there's no follow/fresh behavior. */
export function FileDiff({
  agentId,
  path,
  lang,
  isBuiltInTheme,
}: {
  agentId: string;
  path: string;
  lang: string;
  isBuiltInTheme: boolean;
}) {
  const { hunks, error, loaded } = useFileDiff(agentId, path);

  if (!loaded) {
    return (
      <div className="empty-msg" style={{ margin: "auto" }}>
        <div className="et">Loading diff…</div>
      </div>
    );
  }
  if (error) {
    return (
      <div className="empty-msg" style={{ margin: "auto" }}>
        <div className="et">Couldn't load diff</div>
        <div>Try toggling back and forth, or reopen the file.</div>
      </div>
    );
  }
  if (hunks.length === 0) {
    return (
      <div className="empty-msg" style={{ margin: "auto" }}>
        <div className="et">No textual diff</div>
        <div>This change has no line-level diff to show.</div>
      </div>
    );
  }
  return <DiffBody hunks={hunks} lang={lang} isBuiltInTheme={isBuiltInTheme} />;
}
