// CodeLivePanel — the "Live" mode of the Code panel: an activity feed of the
// agent's edits as unified diffs. Distinct from the Files explorer (browse)
// and from Git (staged changes): it's ordered by edit recency, auto-follows
// the file the agent is currently touching, and animates freshly-arrived lines.
//
// v1 is poll-based: it reuses the 1s git-state poll for the changed-file list
// and fetches each file's diff via `get_file_diff`. True edit-streaming (a
// typing cursor per keystroke) is a deferred follow-up.
import { useEffect, useMemo, useRef, useState } from "react";
import { api, type AgentRecord, type FileStatus } from "../../../api";
import { useAppStore } from "../../../store";
import { usePoll } from "../../../util/hooks";
import { hljsLang } from "../../../data/languages";
import { highlightToHtml } from "../../../util/highlight";
import { useHljsTheme } from "../../../util/codeTheme";
import { parseUnifiedDiff, type DiffHunk, type DiffLine } from "../../../util/diff";
import { Icon } from "../../Icon";

interface CodeLivePanelProps {
  agent: AgentRecord;
  /** The file shared with Files mode; may be null or point at an unchanged file. */
  selectedPath: string | null;
  onSelect: (path: string) => void;
  onOpenInEditor: (path: string) => void;
}

function statusLetter(kind: FileStatus["kind"]): string {
  switch (kind) {
    case "modified":   return "M";
    case "added":      return "A";
    case "deleted":    return "D";
    case "renamed":    return "R";
    case "untracked":  return "?";
    case "conflicted": return "!";
    default:           return "?";
  }
}

const extOf = (path: string) => path.split(".").pop() ?? "";
const sigOf = (f: FileStatus) => `${f.additions}:${f.deletions}`;

export function CodeLivePanel({ agent, selectedPath, onSelect, onOpenInEditor }: CodeLivePanelProps) {
  const gitState = useAppStore((s) => s.gitStates[agent.id] ?? null);
  const fetchGitState = useAppStore((s) => s.fetchGitState);
  // Is the agent mid-turn? Same signal the chat "thinking" spinner uses, so
  // the panel's "live" state appears and clears in lockstep with the rest of
  // the UI. Nothing is "live" once the turn ends.
  const busy = useAppStore((s) => s.managedBusy[agent.id] ?? false);
  // Match the editor's syntax theme: the "quorum" palette is gated by `cq`;
  // other families color `.hljs-*` globally via a loaded stylesheet.
  const isQuorum = useHljsTheme();

  // Reuse the same 1s git poll the Git tab uses; only one right-rail tab is
  // mounted at a time, so this never double-polls.
  const pollGitState = useMemo(() => () => fetchGitState(agent.id), [agent.id, fetchGitState]);
  usePoll(pollGitState, 1000, [pollGitState]);

  const files = useMemo(() => gitState?.files ?? [], [gitState]);
  const fileSig = files.map((f) => `${f.path}#${sigOf(f)}`).join("|");

  // ── recency / "live" detection ──────────────────────────────────────────
  // Without an edit stream we approximate the agent's current file: whichever
  // changed-file's +/- counts moved since the last poll. This only means
  // anything while the agent is mid-turn — when it goes idle, nothing is live.
  const prevSig = useRef<Map<string, string>>(new Map());
  const seeded = useRef(false);
  const [liveFile, setLiveFile] = useState<string | null>(null);

  // Reset recency tracking when the agent changes — the panel stays mounted
  // across agent switches, so stale signatures would mis-detect the live file.
  useEffect(() => {
    seeded.current = false;
    prevSig.current = new Map();
    setLiveFile(null);
  }, [agent.id]);

  useEffect(() => {
    const sig = new Map(files.map((f) => [f.path, sigOf(f)]));

    // Idle: clear the live marker, but keep the baseline current so the next
    // turn's very first edit registers as movement.
    if (!busy) {
      prevSig.current = sig;
      seeded.current = files.length > 0;
      setLiveFile(null);
      return;
    }

    // Turn in progress. Seed on the first poll we have files for: guess the
    // most-changed file as the current one until real movement refines it.
    if (!seeded.current) {
      if (files.length === 0) return;
      seeded.current = true;
      prevSig.current = sig;
      const top = [...files].sort(
        (a, b) => b.additions + b.deletions - (a.additions + a.deletions),
      )[0];
      setLiveFile(top?.path ?? null);
      return;
    }

    let moved: string | null = null;
    for (const f of files) {
      if (prevSig.current.get(f.path) !== sigOf(f)) moved = f.path;
    }
    prevSig.current = sig;
    if (moved) setLiveFile(moved);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [busy, fileSig]);

  // ── follow ────────────────────────────────────────────────────────────
  // When armed, the rendered diff jumps to whatever file the agent is editing
  // now. Start paused only if the user arrived with a file already selected
  // (e.g. via "View diff" from the editor); otherwise follow the agent.
  const [follow, setFollow] = useState(() => selectedPath == null);
  useEffect(() => {
    if (follow && busy && liveFile && liveFile !== selectedPath) onSelect(liveFile);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [follow, busy, liveFile]);

  const changed = useMemo(() => new Set(files.map((f) => f.path)), [files]);
  // The file actually shown: the shared selection if it's a changed file,
  // else the live file, else the first change.
  const displayPath =
    selectedPath && changed.has(selectedPath)
      ? selectedPath
      : liveFile && changed.has(liveFile)
        ? liveFile
        : files[0]?.path ?? null;

  const displaySig = files.find((f) => f.path === displayPath);
  const displaySigStr = displaySig ? sigOf(displaySig) : "";

  // ── diff fetch ──────────────────────────────────────────────────────────
  const [diffText, setDiffText] = useState<string | null>(null);
  const [diffErr, setDiffErr] = useState(false);
  useEffect(() => {
    if (!displayPath) { setDiffText(null); setDiffErr(false); return; }
    let cancelled = false;
    api
      .getFileDiff(agent.id, displayPath)
      .then((t) => { if (!cancelled) { setDiffText(t); setDiffErr(false); } })
      .catch(() => { if (!cancelled) { setDiffText(null); setDiffErr(true); } });
    return () => { cancelled = true; };
  }, [agent.id, displayPath, displaySigStr]);

  const hunks = useMemo(() => (diffText ? parseUnifiedDiff(diffText) : []), [diffText]);

  // ── fresh-line highlighting ──────────────────────────────────────────────
  // Mark added lines that weren't present last time we rendered this same file
  // (a new file view doesn't flash its whole body).
  const prevAdds = useRef<{ path: string; keys: Set<string> }>({ path: "", keys: new Set() });
  const addKey = (l: DiffLine) => `${l.n}:${l.t}`;
  const freshKeys = useMemo(() => {
    const set = new Set<string>();
    if (prevAdds.current.path !== displayPath) return set;
    for (const h of hunks)
      for (const l of h.lines)
        if (l.op === "add" && !prevAdds.current.keys.has(addKey(l))) set.add(addKey(l));
    return set;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hunks, displayPath]);
  useEffect(() => {
    const keys = new Set<string>();
    for (const h of hunks)
      for (const l of h.lines) if (l.op === "add") keys.add(addKey(l));
    prevAdds.current = { path: displayPath ?? "", keys };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hunks, displayPath]);

  // Clicking the file the agent is editing now re-arms auto-follow; clicking
  // any other file pauses it (the user is reading something specific).
  const onPickTab = (path: string) => {
    onSelect(path);
    setFollow(busy && path === liveFile);
  };

  if (files.length === 0) {
    return (
      <div className="code-wrap">
        <div className="empty-msg" style={{ margin: "auto" }}>
          <div className="et">No changes yet</div>
          <div>Edits the agent makes to this worktree will stream in here.</div>
        </div>
      </div>
    );
  }

  const lang = displayPath ? extOf(displayPath) : "";

  return (
    <div className="code-wrap">
      {/* header: totals + follow toggle */}
      <div className="code-h">
        <div className="code-h-l">
          <span className="ch-count">{files.length}<span className="dim"> files</span></span>
          <span className="ch-dot">·</span>
          <span className="ch-add">+{gitState?.additions ?? 0}</span>
          <span className="ch-rem">−{gitState?.deletions ?? 0}</span>
        </div>
        {displayPath && (
          <button
            className="code-open tip"
            data-tip-down
            data-tip="Open this file in the editor"
            onClick={() => onOpenInEditor(displayPath)}
          >
            <Icon name="edit" size={11} />
            <span>Edit</span>
          </button>
        )}
        <button
          className={`code-follow ${follow ? "on" : "off"} ${busy ? "live" : ""} tip`}
          data-tip-down
          data-tip={follow ? "Auto-following the agent's current file" : "Click to auto-follow agent edits"}
          onClick={() => setFollow((v) => !v)}
        >
          <span className="cf-dot"></span>
          <span>{follow ? (busy ? "Following live" : "Following") : "Paused"}</span>
        </button>
      </div>

      {/* file tabs */}
      <div className="code-tabs" role="tablist">
        {files.map((f) => {
          const live = busy && f.path === liveFile;
          const base = f.path.split("/").pop();
          return (
            <button
              key={f.path}
              role="tab"
              className={`code-tab ${f.path === displayPath ? "active" : ""} ${live ? "live" : ""} status-${statusLetter(f.kind).toLowerCase()}`}
              onClick={() => onPickTab(f.path)}
              title={f.path}
            >
              <span className="ct-status">{statusLetter(f.kind)}</span>
              <span className="ct-name">{base}</span>
              <span className="ct-stats">
                {f.additions > 0 && <span className="a">+{f.additions}</span>}
                {f.deletions > 0 && <span className="r">−{f.deletions}</span>}
              </span>
              {live && <span className="ct-live"></span>}
            </button>
          );
        })}
      </div>

      {/* diff */}
      {diffErr ? (
        <div className="empty-msg" style={{ margin: "auto" }}>
          <div className="et">Couldn't load diff</div>
          <div>Retrying on the next change.</div>
        </div>
      ) : hunks.length === 0 ? (
        <div className="empty-msg" style={{ margin: "auto" }}>
          <div className="et">Nothing to show</div>
          <div>This change has no textual diff (binary, or already committed).</div>
        </div>
      ) : (
        <div className={`code-diff ${isQuorum ? "cq" : ""} ${follow && busy && displayPath === liveFile ? "live" : ""}`}>
          {hunks.map((h: DiffHunk, i) => (
            <div key={i}>
              <div className="code-hunk-h">{h.header}</div>
              {h.lines.map((l, j) => (
                <LiveDiffLine key={j} line={l} lang={lang} fresh={l.op === "add" && freshKeys.has(`${l.n}:${l.t}`)} />
              ))}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function LiveDiffLine({ line, lang, fresh }: { line: DiffLine; lang: string; fresh: boolean }) {
  const sigil = line.op === "add" ? "+" : line.op === "rem" ? "−" : " ";
  const html = useMemo(
    () => (line.t ? highlightToHtml(line.t, hljsLang(lang) ? lang : "") : ""),
    [line.t, lang],
  );
  return (
    <div className={`dl op-${line.op}${fresh ? " fresh" : ""}`}>
      <span className="dl-num o">{line.o ?? ""}</span>
      <span className="dl-num n">{line.n ?? ""}</span>
      <span className="dl-sigil">{sigil}</span>
      <span className="dl-text" dangerouslySetInnerHTML={{ __html: html }} />
    </div>
  );
}
