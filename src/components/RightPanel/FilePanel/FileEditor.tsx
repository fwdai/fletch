// The Files-panel editor: a transparent <textarea> over a live
// syntax-highlight layer, with line numbers and a git-style change gutter.
// Edit, ⌘S to save, Revert to restore the agent's version. The "Diff" toggle
// swaps the editor for a read-only unified diff of the agent's changes.
import { type KeyboardEvent, useEffect, useMemo, useRef, useState } from "react";
import { type AgentRecord, api, type WorktreeFileContents } from "../../../api";
import { CODE_THEMES } from "../../../data/codeThemes";
import { langLabel } from "../../../data/languages";
import { useAppStore } from "../../../store";
import { useHljsTheme } from "../../../util/codeTheme";
import { highlightToHtml } from "../../../util/highlight";
import { Icon } from "../../Icon";
import { FileDiff } from "../Code/DiffView";
import { ViewerHeader } from "./ViewerHeader";

interface FileEditorProps {
  agent: AgentRecord;
  path: string;
  name: string;
  dir: string;
  file: WorktreeFileContents;
  onBack: () => void;
}

export function FileEditor({ agent, path, name, dir, file, onBack }: FileEditorProps) {
  const originalText = file.text;
  const [value, setValue] = useState(originalText);
  // Files auto-save a short beat after you stop typing — no Save button. This
  // drives the quiet status line; `savedRef` tracks what's currently on disk.
  const [saveState, setSaveState] = useState<"idle" | "saving" | "saved" | "error">("idle");
  // "code" = editable view; "diff" = read-only unified diff of agent changes.
  const [view, setView] = useState<"code" | "diff">("code");
  // Only changed files have a diff worth showing.
  const canDiff = !!file.status;
  const diffView = view === "diff" && canDiff;

  // Syntax theme: "quorum" uses our palette (gated by the `cq` class); other
  // families load a highlight.js stylesheet that follows the app's dark/light.
  const isQuorum = useHljsTheme();
  const codeTheme = useAppStore((s) => s.codeTheme);
  const setCodeTheme = useAppStore((s) => s.setCodeTheme);
  const setLastError = useAppStore((s) => s.setLastError);

  const taRef = useRef<HTMLTextAreaElement>(null);
  const hlRef = useRef<HTMLPreElement>(null);
  const gutRef = useRef<HTMLDivElement>(null);
  // Autosave bookkeeping: last text written to disk, the latest buffer (for
  // the unmount flush), and the pending debounce timer.
  const savedRef = useRef(originalText);
  const valueRef = useRef(value);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    valueRef.current = value;
  }, [value]);

  const lines = value.split("\n");
  const origLines = useMemo(() => originalText.split("\n"), [originalText]);
  const editedFromAgent = value !== originalText;
  const isDeleted = file.status === "D";

  const addSet = useMemo(() => new Set(file.chg_add), [file.chg_add]);
  const modSet = useMemo(() => new Set(file.chg_mod), [file.chg_mod]);

  const html = useMemo(() => {
    const out = highlightToHtml(value, file.lang);
    // A trailing newline renders an empty final line in the <textarea> but NOT
    // in a <pre>, leaving the highlight layer one line shorter — so it
    // under-scrolls at the bottom and the caret drifts above the code toward
    // EOF. Mirror that empty line so both layers are exactly the same height.
    return value.endsWith("\n") ? `${out}\n` : out;
  }, [value, file.lang]);

  // change gutter: at the agent's version → show its markers; once you diverge
  // → mark the lines you changed (vs the agent's version).
  const lineKind = (i: number): "add" | "mod" | "rem" | null => {
    if (!editedFromAgent) {
      if (file.status === "A") return "add";
      if (isDeleted) return "rem";
      if (addSet.has(i + 1)) return "add";
      if (modSet.has(i + 1)) return "mod";
      return null;
    }
    if (i >= origLines.length) return "add";
    if (lines[i] !== origLines[i]) return "mod";
    return null;
  };

  const syncScroll = () => {
    const ta = taRef.current;
    if (!ta) return;
    if (hlRef.current) {
      hlRef.current.scrollTop = ta.scrollTop;
      hlRef.current.scrollLeft = ta.scrollLeft;
    }
    if (gutRef.current) gutRef.current.scrollTop = ta.scrollTop;
  };

  // Write the latest buffer to disk if it differs from what's there.
  const flush = async () => {
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    const text = valueRef.current;
    if (text === savedRef.current) return;
    setSaveState("saving");
    try {
      await api.writeWorktreeFile(agent.id, path, text);
      savedRef.current = text;
      // Only settle to "saved" if no newer keystroke landed mid-write.
      setSaveState(valueRef.current === text ? "saved" : "saving");
    } catch {
      setSaveState("error");
    }
  };

  // Debounce a save after typing stops.
  const scheduleSave = () => {
    setSaveState("saving");
    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = setTimeout(() => {
      void flush();
    }, 600);
  };

  // Let the "Saved" tick fade back to idle.
  useEffect(() => {
    if (saveState !== "saved") return;
    const t = setTimeout(() => setSaveState("idle"), 1300);
    return () => clearTimeout(t);
  }, [saveState]);

  // Flush any pending edit when the editor closes or switches files, so a save
  // mid-debounce is never lost.
  useEffect(() => {
    return () => {
      if (timerRef.current) clearTimeout(timerRef.current);
      const text = valueRef.current;
      if (text !== savedRef.current) {
        // The component is unmounting, so local state is gone — surface a
        // failed final save through the global banner instead of losing it
        // silently. `getState()` avoids a stale closure in cleanup.
        void api.writeWorktreeFile(agent.id, path, text).catch((e) => {
          useAppStore.getState().setLastError(`Couldn't save ${path}: ${String(e)}`);
        });
      }
    };
  }, [agent.id, path]);

  const revert = async () => {
    // Restore the agent's version on disk and in the editor.
    if (timerRef.current) {
      clearTimeout(timerRef.current);
      timerRef.current = null;
    }
    setSaveState("saving");
    try {
      await api.writeWorktreeFile(agent.id, path, originalText);
      savedRef.current = originalText;
      setValue(originalText);
      setSaveState("idle");
      requestAnimationFrame(syncScroll);
    } catch (e) {
      // Don't update the buffer/savedRef — disk still holds the edits, so the
      // editor must keep showing them rather than claim a revert that failed.
      setSaveState("error");
      setLastError(`Couldn't revert ${name}: ${String(e)}`);
    }
  };

  const onChange = (next: string) => {
    setValue(next);
    scheduleSave();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key.toLowerCase() === "s") {
      e.preventDefault();
      void flush(); // ⌘S just flushes the pending autosave immediately
      return;
    }
    if (e.key === "Tab") {
      e.preventDefault();
      const ta = e.currentTarget;
      const s = ta.selectionStart;
      const en = ta.selectionEnd;
      const next = `${value.slice(0, s)}  ${value.slice(en)}`;
      onChange(next);
      requestAnimationFrame(() => {
        ta.selectionStart = ta.selectionEnd = s + 2;
      });
    }
  };

  return (
    <div className="fp-wrap">
      <ViewerHeader
        name={name}
        dir={dir}
        onBack={onBack}
        status={file.status}
        dirty={saveState === "saving"}
        actions={
          canDiff ? (
            <span className="fp-vh-actions">
              <button
                className={`fp-meta-btn ${diffView ? "active" : ""}`}
                title={diffView ? "Back to the file" : "Show what the agent changed"}
                onClick={() => setView((v) => (v === "diff" ? "code" : "diff"))}
              >
                <Icon name="diff" size={11} /> Diff
              </button>
            </span>
          ) : undefined
        }
      />

      {diffView ? (
        <FileDiff agentId={agent.id} path={path} lang={file.lang} isQuorum={isQuorum} />
      ) : (
        <>
          {isDeleted && (
            <div className="fp-banner del">
              <Icon name="trash" size={12} />
              <span>Deleted by the agent. Edits here re-create the file.</span>
            </div>
          )}
          {file.status === "A" && value === originalText && (
            <div className="fp-banner add">
              <Icon name="plus" size={12} />
              <span>New file added by the agent.</span>
            </div>
          )}

          <div className="fp-editor">
            <div className="fp-gutter" ref={gutRef}>
              <div className="fp-gutter-inner">
                {lines.map((_, i) => {
                  const k = lineKind(i);
                  return (
                    <div className="fp-gline" key={i}>
                      <span className="fp-num">{i + 1}</span>
                      <span className={`fp-bar ${k || ""}`}></span>
                    </div>
                  );
                })}
              </div>
            </div>
            <div className="fp-edit-main">
              <pre className={`fp-hl ${isQuorum ? "cq" : ""}`} ref={hlRef} aria-hidden="true">
                <code dangerouslySetInnerHTML={{ __html: html }} />
              </pre>
              <textarea
                ref={taRef}
                className="fp-ta"
                value={value}
                wrap="off"
                spellCheck={false}
                autoCapitalize="off"
                autoCorrect="off"
                onChange={(e) => onChange(e.target.value)}
                onScroll={syncScroll}
                onKeyDown={onKeyDown}
              />
            </div>
          </div>
        </>
      )}

      <div className="fp-meta">
        <span className="fp-meta-lang">{langLabel(file.lang)}</span>
        {diffView ? (
          <>
            <span className="fp-meta-dot">·</span>
            <span>Diff vs {file.status === "A" ? "new file" : "base"}</span>
          </>
        ) : (
          <>
            <span className="fp-meta-dot">·</span>
            <span>{lines.length} lines</span>
            {saveState === "saving" ? (
              <>
                <span className="fp-meta-dot">·</span>
                <span className="fp-meta-edited">Saving…</span>
              </>
            ) : saveState === "saved" ? (
              <>
                <span className="fp-meta-dot">·</span>
                <span className="fp-meta-saved">Saved ✓</span>
              </>
            ) : saveState === "error" ? (
              <>
                <span className="fp-meta-dot">·</span>
                <span className="fp-meta-failed">Save failed</span>
              </>
            ) : editedFromAgent ? (
              <>
                <span className="fp-meta-dot">·</span>
                <span className="fp-meta-edited">Edited</span>
              </>
            ) : null}
          </>
        )}
        <span className="fp-meta-grow"></span>
        {!diffView && editedFromAgent && (
          <button
            className="fp-meta-btn"
            title="Discard edits, restore the agent's version"
            onClick={() => void revert()}
          >
            <Icon name="refresh" size={11} /> Revert
          </button>
        )}
        <select
          className="fp-theme-select"
          value={codeTheme}
          onChange={(e) => setCodeTheme(e.target.value)}
          title="Syntax highlighting theme"
        >
          {CODE_THEMES.map((t) => (
            <option key={t.id} value={t.id}>
              {t.label}
            </option>
          ))}
        </select>
      </div>
    </div>
  );
}
