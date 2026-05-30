// FilePanel — the right-rail "Files" tab. Browse the agent's worktree and
// view/edit any file's contents (NOT diffs — that's the Diff panel's job).
//
// Two modes in one narrow rail:
//   • Explorer — a VS Code-style tree. Agent-touched files carry a git
//     status + colored name; a "Changed" filter and a search box narrow it.
//   • Editor   — a transparent <textarea> over a live syntax-highlight layer,
//     with line numbers and a git-style change gutter. Edit, ⌘S to save,
//     Revert to restore the agent's version.
//
// Faithful port of the design (quorum v2 files.jsx), wired to the real
// worktree via the `*_worktree_*` Tauri commands.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type AgentRecord, type WorktreeFile, type WorktreeFileContents } from "../../api";
import { useAppStore } from "../../store";
import { CODE_THEMES } from "../../data/codeThemes";
import { usePoll } from "../../util/hooks";
import { highlightToHtml } from "../../util/highlight";
import { useHljsTheme } from "../../util/codeTheme";
import { langLabel } from "../../data/languages";
import { FileIcon } from "./FileIcon";
import { Icon } from "../Icon";

// ── tree model ──────────────────────────────────────────────────────────
type DirNode = { type: "dir"; name: string; path: string; children: TreeNode[] };
type FileNode = {
  type: "file";
  name: string;
  path: string;
  status: string | null;
  additions: number;
  deletions: number;
};
type TreeNode = DirNode | FileNode;

interface FilePanelProps {
  agent: AgentRecord;
  canViewDiff: boolean;
  onViewDiff: () => void;
}

export function FilePanel({ agent, canViewDiff, onViewDiff }: FilePanelProps) {
  const [files, setFiles] = useState<WorktreeFile[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [openPath, setOpenPath] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [changedOnly, setChangedOnly] = useState(false);
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());

  // Reset per-agent so one worktree's open file / search doesn't leak.
  useEffect(() => {
    setOpenPath(null);
    setQuery("");
    setChangedOnly(false);
    setLoaded(false);
    setFiles([]);
    setExpanded(new Set());
  }, [agent.id]);

  const refresh = useCallback(async () => {
    try {
      setFiles(await api.listWorktreeTree(agent.id));
    } catch {
      // Keep the previous tree on a transient IPC error rather than blanking it.
    }
    setLoaded(true);
  }, [agent.id]);

  // Poll the tree at 2s, but only while the explorer is showing — no point
  // re-listing the worktree while a file is open in the editor.
  const pollTree = useCallback(async () => {
    if (!openPath) await refresh();
  }, [openPath, refresh]);
  usePoll(pollTree, 2000, [pollTree]);

  const fullTree = useMemo(() => buildTree(files), [files]);

  // Default every directory open the first time files arrive.
  const seededRef = useRef(false);
  useEffect(() => {
    if (!seededRef.current && files.length) {
      seededRef.current = true;
      setExpanded(new Set(allDirPaths(fullTree)));
    }
  }, [files.length, fullTree]);
  useEffect(() => { seededRef.current = false; }, [agent.id]);

  const changedCount = useMemo(() => files.filter((f) => f.status).length, [files]);
  const filtering = query.trim() !== "" || changedOnly;
  const tree = filtering
    ? filterTree(fullTree, query.trim().toLowerCase(), changedOnly)
    : fullTree;

  const toggleDir = (path: string) =>
    setExpanded((s) => {
      const n = new Set(s);
      if (n.has(path)) n.delete(path);
      else n.add(path);
      return n;
    });

  // ── editor ──────────────────────────────────────────────────────────
  if (openPath) {
    return (
      <FileViewer
        key={openPath}
        agent={agent}
        path={openPath}
        canViewDiff={canViewDiff}
        onViewDiff={onViewDiff}
        onBack={() => { setOpenPath(null); void refresh(); }}
      />
    );
  }

  // ── explorer ────────────────────────────────────────────────────────
  return (
    <div className="fp-wrap">
      <div className="fp-toolbar">
        <div className="fp-search">
          <Icon name="search" size={12} />
          <input
            type="text"
            placeholder="Search files"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            spellCheck={false}
          />
          {query && (
            <button className="fp-clear" onClick={() => setQuery("")} aria-label="Clear">
              <Icon name="close" size={10} />
            </button>
          )}
        </div>
        <button
          className={`fp-filter ${changedOnly ? "on" : ""}`}
          title={changedOnly ? "Showing changed files only" : "Show only files the agent changed"}
          onClick={() => setChangedOnly((v) => !v)}
        >
          <span className="fp-filter-dot"></span>
          Changed
          <span className="fp-filter-count">{changedCount}</span>
        </button>
        <button
          className="btn-i xs"
          title="Collapse all"
          onClick={() => setExpanded(new Set())}
        >
          <Icon name="shrink" />
        </button>
      </div>

      <div className="fp-tree">
        {tree.length === 0 ? (
          <div className="empty-msg" style={{ margin: "auto" }}>
            <div className="et">{loaded ? "No matching files" : "Loading…"}</div>
            {loaded && (
              <div>
                {changedOnly ? "Clear the Changed filter or " : ""}adjust your search.
              </div>
            )}
          </div>
        ) : (
          tree.map((node) => (
            <TreeRow
              key={node.path}
              node={node}
              depth={0}
              expanded={expanded}
              forceOpen={filtering}
              onToggle={toggleDir}
              onOpen={setOpenPath}
            />
          ))
        )}
      </div>
    </div>
  );
}

// ── tree row (recursive) ──────────────────────────────────────────────────
interface TreeRowProps {
  node: TreeNode;
  depth: number;
  expanded: Set<string>;
  forceOpen: boolean;
  onToggle: (path: string) => void;
  onOpen: (path: string) => void;
}

function TreeRow({ node, depth, expanded, forceOpen, onToggle, onOpen }: TreeRowProps) {
  const pad = 10 + depth * 13;

  if (node.type === "dir") {
    const isOpen = forceOpen || expanded.has(node.path);
    return (
      <>
        <button className="fp-row dir" style={{ paddingLeft: pad }} onClick={() => onToggle(node.path)}>
          <span className={`fp-twisty ${isOpen ? "open" : ""}`}>
            <Icon name="chevR" size={11} />
          </span>
          <FileIcon name={node.name} folder open={isOpen} />
          <span className="fp-name">{node.name}</span>
        </button>
        {isOpen &&
          node.children.map((c) => (
            <TreeRow
              key={c.path}
              node={c}
              depth={depth + 1}
              expanded={expanded}
              forceOpen={forceOpen}
              onToggle={onToggle}
              onOpen={onOpen}
            />
          ))}
      </>
    );
  }

  const st = node.status ? node.status.toLowerCase() : "";
  return (
    <button
      className={`fp-row file ${node.status ? "changed s-" + st : ""}`}
      style={{ paddingLeft: pad + 13 }}
      onClick={() => onOpen(node.path)}
      title={node.path}
    >
      <FileIcon name={node.name} />
      <span className="fp-name">{node.name}</span>
      {node.status && <span className={`fp-badge s-${st}`}>{node.status}</span>}
    </button>
  );
}

// ── file viewer / editor ────────────────────────────────────────────────────
interface FileViewerProps {
  agent: AgentRecord;
  path: string;
  canViewDiff: boolean;
  onViewDiff: () => void;
  onBack: () => void;
}

function FileViewer({ agent, path, canViewDiff, onViewDiff, onBack }: FileViewerProps) {
  const parts = path.split("/");
  const name = parts.pop() as string;
  const dir = parts.join("/");

  const [contents, setContents] = useState<WorktreeFileContents | null>(null);
  const [error, setError] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setContents(null);
    setError(false);
    api
      .readWorktreeFile(agent.id, path)
      .then((c) => { if (!cancelled) setContents(c); })
      .catch(() => { if (!cancelled) setError(true); });
    return () => { cancelled = true; };
  }, [agent.id, path]);

  if (error || (contents && (contents.binary || contents.too_large))) {
    return (
      <div className="fp-wrap">
        <ViewerHeader name={name} dir={dir} onBack={onBack} status={contents?.status ?? null} dirty={false} />
        <div className="empty-msg" style={{ margin: "auto" }}>
          <div className="et">No preview</div>
          <div>
            {contents?.too_large
              ? "This file is too large to show here."
              : contents?.binary
                ? "This is a binary file."
                : "This file can't be shown here."}
          </div>
        </div>
      </div>
    );
  }

  if (!contents) {
    return (
      <div className="fp-wrap">
        <ViewerHeader name={name} dir={dir} onBack={onBack} status={null} dirty={false} />
        <div className="empty-msg" style={{ margin: "auto" }}>
          <div className="et">Loading…</div>
        </div>
      </div>
    );
  }

  return (
    <FileEditor
      key={path}
      agent={agent}
      path={path}
      name={name}
      dir={dir}
      file={contents}
      canViewDiff={canViewDiff}
      onViewDiff={onViewDiff}
      onBack={onBack}
    />
  );
}

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

function FileEditor({ agent, path, name, dir, file, canViewDiff, onViewDiff, onBack }: FileEditorProps) {
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

interface ViewerHeaderProps {
  name: string;
  dir: string;
  status: string | null;
  dirty: boolean;
  onBack: () => void;
  actions?: React.ReactNode;
}

function ViewerHeader({ name, dir, status, dirty, onBack, actions }: ViewerHeaderProps) {
  const st = status ? status.toLowerCase() : "";
  return (
    <div className="fp-viewer-h">
      <button className="fp-back" title="Back to files" onClick={onBack}>
        <Icon name="chevL" size={13} />
      </button>
      <FileIcon name={name} />
      <div className="fp-crumb">
        {dir && <span className="fp-crumb-dir">{dir}/</span>}
        <span className="fp-crumb-file">{name}</span>
        {dirty && <span className="fp-crumb-dot" title="Unsaved changes"></span>}
      </div>
      {status && <span className={`fp-badge s-${st}`}>{status}</span>}
      {actions}
    </div>
  );
}

// ── helpers ────────────────────────────────────────────────────────────────

/** Build a sorted nested tree (dirs first, then files; alpha within each)
 *  from a flat list of worktree files. */
function buildTree(files: WorktreeFile[]): TreeNode[] {
  const roots: TreeNode[] = [];
  // path → DirNode, so we can attach children as we walk each file's segments.
  const dirIndex = new Map<string, DirNode>();

  const childrenOf = (path: string): TreeNode[] =>
    path === "" ? roots : (dirIndex.get(path) as DirNode).children;

  for (const f of files) {
    const segs = f.path.split("/");
    let prefix = "";
    // create intermediate directories
    for (let i = 0; i < segs.length - 1; i++) {
      const parent = prefix;
      prefix = prefix ? `${prefix}/${segs[i]}` : segs[i];
      if (!dirIndex.has(prefix)) {
        const node: DirNode = { type: "dir", name: segs[i], path: prefix, children: [] };
        dirIndex.set(prefix, node);
        childrenOf(parent).push(node);
      }
    }
    const parent = segs.length > 1 ? segs.slice(0, -1).join("/") : "";
    childrenOf(parent).push({
      type: "file",
      name: segs[segs.length - 1],
      path: f.path,
      status: f.status,
      additions: f.additions,
      deletions: f.deletions,
    });
  }

  sortNodes(roots);
  return roots;
}

function sortNodes(nodes: TreeNode[]): void {
  nodes.sort((a, b) => {
    if (a.type !== b.type) return a.type === "dir" ? -1 : 1;
    return a.name.localeCompare(b.name);
  });
  for (const n of nodes) if (n.type === "dir") sortNodes(n.children);
}

function allDirPaths(nodes: TreeNode[], acc: string[] = []): string[] {
  for (const n of nodes) {
    if (n.type === "dir") {
      acc.push(n.path);
      allDirPaths(n.children, acc);
    }
  }
  return acc;
}

/** Pruned copy containing only files that match the query / changed filter
 *  (and their ancestor directories). */
function filterTree(nodes: TreeNode[], q: string, changedOnly: boolean): TreeNode[] {
  const out: TreeNode[] = [];
  for (const n of nodes) {
    if (n.type === "dir") {
      const kids = filterTree(n.children, q, changedOnly);
      if (kids.length) out.push({ ...n, children: kids });
    } else {
      const nameOk = !q || n.name.toLowerCase().includes(q);
      const chgOk = !changedOnly || !!n.status;
      if (nameOk && chgOk) out.push(n);
    }
  }
  return out;
}
