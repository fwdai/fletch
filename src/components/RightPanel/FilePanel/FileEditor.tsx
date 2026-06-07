// The Files-panel editor: a transparent <textarea> over a live
// syntax-highlight layer, with line numbers and a git-style change gutter.
// Edit, ⌘S to save, Revert to restore the agent's version.
import { useMemo, useRef, useState } from "react";
import { api, type AgentRecord, type WorktreeFileContents } from "../../../api";
import { useAppStore } from "../../../store";
import { CODE_THEMES } from "../../../data/codeThemes";
import { highlightToHtml } from "../../../util/highlight";
import { useHljsTheme } from "../../../util/codeTheme";
import { langLabel } from "../../../data/languages";
import { Icon } from "../../Icon";
import { ViewerHeader } from "./ViewerHeader";

interface FileEditorProps {
  agent: AgentRecord;
  path: string;
  name: string;
  dir: string;
  file: WorktreeFileContents;
  canViewDiff: boolean;
  onViewDiff: () => void;
  onBack: () => void;
}

export function FileEditor({ agent, path, name, dir, file, canViewDiff, onViewDiff, onBack }: FileEditorProps) {
  const originalText = file.text;
  const [value, setValue] = useState(originalText);
  const [baseline, setBaseline] = useState(originalText);
  const [copied, setCopied] = useState(false);
  const [flash, setFlash] = useState(false); // "saved" pulse
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState(false);

  // Syntax theme: "quorum" uses our palette (gated by the `cq` class); other
  // families load a highlight.js stylesheet that follows the app's dark/light.
  const isQuorum = useHljsTheme();
  const codeTheme = useAppStore((s) => s.codeTheme);
  const setCodeTheme = useAppStore((s) => s.setCodeTheme);

  const taRef = useRef<HTMLTextAreaElement>(null);
  const hlRef = useRef<HTMLPreElement>(null);
  const gutRef = useRef<HTMLDivElement>(null);

  const lines = value.split("\n");
  const baseLines = baseline.split("\n");
  const dirty = value !== baseline;
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

  // change gutter: pristine → show the agent's markers; editing → mark your edits
  const lineKind = (i: number): "add" | "mod" | "rem" | null => {
    if (!dirty) {
      if (file.status === "A" && value === originalText) return "add";
      if (isDeleted) return "rem";
      if (addSet.has(i + 1)) return "add";
      if (modSet.has(i + 1)) return "mod";
      return null;
    }
    if (i >= baseLines.length) return "add";
    if (lines[i] !== baseLines[i]) return "mod";
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

  const save = async () => {
    if (saving) return;
    setSaving(true);
    setSaveError(false);
    try {
      await api.writeWorktreeFile(agent.id, path, value);
      setBaseline(value);
      setFlash(true);
      setTimeout(() => setFlash(false), 1300);
    } catch {
      setSaveError(true);
    } finally {
      setSaving(false);
    }
  };

  const revert = async () => {
    // Restore the agent's version on disk and in the editor.
    await api.writeWorktreeFile(agent.id, path, originalText).catch(() => {});
    setValue(originalText);
    setBaseline(originalText);
    requestAnimationFrame(syncScroll);
  };

  const copy = () => {
    navigator.clipboard?.writeText(value).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 1400);
  };

  const onKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key.toLowerCase() === "s") {
      e.preventDefault();
      if (dirty) void save();
      return;
    }
    if (e.key === "Tab") {
      e.preventDefault();
      const ta = e.currentTarget;
      const s = ta.selectionStart;
      const en = ta.selectionEnd;
      const next = value.slice(0, s) + "  " + value.slice(en);
      setValue(next);
      requestAnimationFrame(() => { ta.selectionStart = ta.selectionEnd = s + 2; });
    }
  };

  return (
    <div className={`fp-wrap ${dirty ? "is-dirty" : ""}`}>
      <ViewerHeader
        name={name}
        dir={dir}
        onBack={onBack}
        status={file.status}
        dirty={dirty}
        actions={
          <span className="fp-vh-actions">
            <button className="fp-meta-btn" onClick={copy}>
              <Icon name={copied ? "check" : "copy"} size={11} />
              {copied ? "Copied" : "Copy"}
            </button>
            <button className={`fp-save ${dirty ? "" : "disabled"}`} onClick={() => void save()} disabled={!dirty}>
              <Icon name="check" size={11} /> Save
              <span className="fp-save-kbd">⌘S</span>
            </button>
          </span>
        }
      />

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
            onChange={(e) => { setValue(e.target.value); setSaveError(false); }}
            onScroll={syncScroll}
            onKeyDown={onKeyDown}
          />
        </div>
      </div>

      <div className="fp-meta">
        <span className="fp-meta-lang">{langLabel(file.lang)}</span>
        <span className="fp-meta-dot">·</span>
        <span>{lines.length} lines</span>
        {dirty ? (
          <><span className="fp-meta-dot">·</span><span className="fp-meta-unsaved">Unsaved</span></>
        ) : flash ? (
          <><span className="fp-meta-dot">·</span><span className="fp-meta-saved">Saved ✓</span></>
        ) : editedFromAgent ? (
          <><span className="fp-meta-dot">·</span><span className="fp-meta-edited">Edited</span></>
        ) : null}
        {saveError && (
          <><span className="fp-meta-dot">·</span><span className="fp-meta-failed">Save failed</span></>
        )}
        <span className="fp-meta-grow"></span>
        {editedFromAgent && (
          <button className="fp-meta-btn" title="Discard edits, restore the agent's version" onClick={() => void revert()}>
            <Icon name="refresh" size={11} /> Revert
          </button>
        )}
        {canViewDiff && file.status && file.status !== "A" && file.status !== "D" && (
          <button className="fp-meta-btn" onClick={onViewDiff}>
            <Icon name="diff" size={11} /> Diff
          </button>
        )}
        <select
          className="fp-theme-select"
          value={codeTheme}
          onChange={(e) => setCodeTheme(e.target.value)}
          title="Syntax highlighting theme"
        >
          {CODE_THEMES.map((t) => (
            <option key={t.id} value={t.id}>{t.label}</option>
          ))}
        </select>
      </div>
    </div>
  );
}
