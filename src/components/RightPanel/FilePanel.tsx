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
import { FileContextMenu, type ContextMenuEntry } from "./FileContextMenu";
import { Icon } from "../Icon";
import { basename, joinPath, parentDir } from "../../util/format";

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

// An in-progress inline edit: renaming an existing node, or creating a new
// file/folder inside `parentDir` ("" = repo root).
type EditState =
  | { mode: "rename"; path: string; isDir: boolean }
  | { mode: "newFile" | "newFolder"; parentDir: string };

// An open context menu, anchored at the cursor over `node`.
type MenuState = { x: number; y: number; node: TreeNode };

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
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [edit, setEdit] = useState<EditState | null>(null);
  const [opError, setOpError] = useState<string | null>(null);
  // Newly-created directories that are still empty. git lists files, not dirs,
  // so without this a brand-new folder would vanish on the next poll.
  const [pendingDirs, setPendingDirs] = useState<Set<string>>(() => new Set());

  // Reset per-agent so one worktree's open file / search doesn't leak.
  useEffect(() => {
    setOpenPath(null);
    setQuery("");
    setChangedOnly(false);
    setLoaded(false);
    setFiles([]);
    setExpanded(new Set());
    setMenu(null);
    setEdit(null);
    setOpError(null);
    setPendingDirs(new Set());
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

  const fullTree = useMemo(
    () => buildTree(files, [...pendingDirs]),
    [files, pendingDirs],
  );

  // Once a real file lands inside a tracked empty-dir, git lists the dir via
  // that file, so we can stop injecting it (and keep the set from growing).
  useEffect(() => {
    setPendingDirs((s) => {
      if (!s.size) return s;
      const next = new Set(
        [...s].filter((d) => !files.some((file) => file.path === d || file.path.startsWith(`${d}/`))),
      );
      return next.size === s.size ? s : next;
    });
  }, [files]);

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

  const expand = (path: string) =>
    setExpanded((s) => new Set(s).add(path));

  // ── file operations ───────────────────────────────────────────────────
  const allPaths = useMemo(() => new Set(files.map((f) => f.path)), [files]);

  // Begin a create: open the target dir so the inline input is visible.
  const beginCreate = (mode: "newFile" | "newFolder", dir: string) => {
    setOpError(null);
    if (dir) expand(dir);
    setEdit({ mode, parentDir: dir });
  };

  const cancelEdit = () => setEdit(null);

  // Commit the active inline edit. `value` is the raw input; empty / unchanged
  // values quietly cancel. On a backend error we surface it and keep editing.
  const commitEdit = async (value: string) => {
    const name = value.trim();
    if (!edit) return;
    if (!name) { setEdit(null); return; }
    setOpError(null);
    try {
      if (edit.mode === "rename") {
        const from = edit.path;
        const dest = joinPath(parentDir(from), name);
        if (dest === from) { setEdit(null); return; }
        await api.renameWorktreePath(agent.id, from, dest);
        // An empty folder we're tracking moves with the rename; re-point it (and
        // any tracked descendants) or it would vanish and leave a phantom.
        if (edit.isDir) {
          setPendingDirs((s) => {
            const n = new Set<string>();
            for (const d of s) {
              if (d === from) n.add(dest);
              else if (d.startsWith(`${from}/`)) n.add(dest + d.slice(from.length));
              else n.add(d);
            }
            return n;
          });
        }
      } else if (edit.mode === "newFile") {
        const dest = joinPath(edit.parentDir, name);
        await api.createWorktreeFile(agent.id, dest);
        setEdit(null);
        await refresh();
        setOpenPath(dest);
        return;
      } else {
        const dest = joinPath(edit.parentDir, name);
        await api.createWorktreeDir(agent.id, dest);
        setPendingDirs((s) => new Set(s).add(dest));
        expand(dest);
      }
      setEdit(null);
      await refresh();
    } catch (e) {
      setOpError(errMsg(e));
    }
  };

  const doDelete = async (node: TreeNode) => {
    setOpError(null);
    try {
      await api.deleteWorktreePath(agent.id, node.path);
      // Drop any tracked empty-dir under what we just removed.
      setPendingDirs((s) => {
        const n = new Set(s);
        for (const d of n) {
          if (d === node.path || d.startsWith(`${node.path}/`)) n.delete(d);
        }
        return n;
      });
      await refresh();
    } catch (e) {
      setOpError(errMsg(e));
    }
  };

  const doDuplicate = async (node: FileNode) => {
    setOpError(null);
    try {
      const dest = duplicatePath(node.path, allPaths);
      await api.copyWorktreeFile(agent.id, node.path, dest);
      await refresh();
    } catch (e) {
      setOpError(errMsg(e));
    }
  };

  const copyPath = (node: TreeNode) => {
    navigator.clipboard?.writeText(node.path).catch(() => {});
  };

  // Build the context-menu entries for a right-clicked node.
  const menuEntries = (node: TreeNode): ContextMenuEntry[] => {
    // New File / New Folder target the folder itself, or a file's parent dir.
    const target = node.type === "dir" ? node.path : parentDir(node.path);
    const newItems: ContextMenuEntry[] = [
      { icon: "file", label: "New File…", onClick: () => beginCreate("newFile", target) },
      { icon: "folder", label: "New Folder…", onClick: () => beginCreate("newFolder", target) },
    ];
    const common: ContextMenuEntry[] = [
      { icon: "edit", label: "Rename…", onClick: () => { setOpError(null); setEdit({ mode: "rename", path: node.path, isDir: node.type === "dir" }); } },
      { icon: "copy", label: "Copy Path", onClick: () => copyPath(node) },
    ];
    const del: ContextMenuEntry = {
      icon: "trash",
      label: "Delete",
      danger: true,
      confirmLabel: node.type === "dir" ? "Delete folder & contents?" : "Confirm Delete?",
      onClick: () => void doDelete(node),
    };
    if (node.type === "dir") {
      return [...newItems, "sep", ...common, "sep", del];
    }
    return [
      ...common,
      { icon: "copy", label: "Duplicate", onClick: () => void doDuplicate(node) },
      "sep",
      ...newItems,
      "sep",
      del,
    ];
  };

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
          title="New file"
          onClick={() => beginCreate("newFile", "")}
        >
          <Icon name="file" />
        </button>
        <button
          className="btn-i xs"
          title="New folder"
          onClick={() => beginCreate("newFolder", "")}
        >
          <Icon name="folder" />
        </button>
        <button
          className="btn-i xs"
          title="Collapse all"
          onClick={() => setExpanded(new Set())}
        >
          <Icon name="shrink" />
        </button>
      </div>

      {opError && (
        <div className="fp-op-error" role="alert">
          <Icon name="close" size={11} />
          <span>{opError}</span>
          <button className="fp-clear" onClick={() => setOpError(null)} aria-label="Dismiss">
            <Icon name="close" size={10} />
          </button>
        </div>
      )}

      <div className="fp-tree">
        {edit && edit.mode !== "rename" && edit.parentDir === "" && (
          <CreateRow mode={edit.mode} depth={0} onCommit={commitEdit} onCancel={cancelEdit} />
        )}
        {tree.length === 0 && !edit ? (
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
              edit={edit}
              onToggle={toggleDir}
              onOpen={setOpenPath}
              onMenu={(n, x, y) => setMenu({ node: n, x, y })}
              onCommit={commitEdit}
              onCancel={cancelEdit}
            />
          ))
        )}
      </div>

      {menu && (
        <FileContextMenu
          x={menu.x}
          y={menu.y}
          entries={menuEntries(menu.node)}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}

// ── tree row (recursive) ──────────────────────────────────────────────────
interface TreeRowProps {
  node: TreeNode;
  depth: number;
  expanded: Set<string>;
  forceOpen: boolean;
  edit: EditState | null;
  onToggle: (path: string) => void;
  onOpen: (path: string) => void;
  onMenu: (node: TreeNode, x: number, y: number) => void;
  onCommit: (value: string) => void;
  onCancel: () => void;
}

function TreeRow({
  node, depth, expanded, forceOpen, edit, onToggle, onOpen, onMenu, onCommit, onCancel,
}: TreeRowProps) {
  const pad = 10 + depth * 13;
  const renaming = edit?.mode === "rename" && edit.path === node.path;

  if (node.type === "dir") {
    const isOpen = forceOpen || expanded.has(node.path);
    const creatingHere =
      edit && edit.mode !== "rename" && edit.parentDir === node.path;
    return (
      <>
        <button
          className="fp-row dir"
          style={{ paddingLeft: pad }}
          onClick={() => onToggle(node.path)}
          onContextMenu={(e) => { e.preventDefault(); onMenu(node, e.clientX, e.clientY); }}
        >
          <span className={`fp-twisty ${isOpen ? "open" : ""}`}>
            <Icon name="chevR" size={11} />
          </span>
          <FileIcon name={node.name} folder open={isOpen} />
          {renaming ? (
            <NameInput initial={node.name} onCommit={onCommit} onCancel={onCancel} />
          ) : (
            <span className="fp-name">{node.name}</span>
          )}
        </button>
        {(isOpen || creatingHere) && (
          <>
            {creatingHere && edit && (
              <CreateRow mode={edit.mode} depth={depth + 1} onCommit={onCommit} onCancel={onCancel} />
            )}
            {isOpen &&
              node.children.map((c) => (
                <TreeRow
                  key={c.path}
                  node={c}
                  depth={depth + 1}
                  expanded={expanded}
                  forceOpen={forceOpen}
                  edit={edit}
                  onToggle={onToggle}
                  onOpen={onOpen}
                  onMenu={onMenu}
                  onCommit={onCommit}
                  onCancel={onCancel}
                />
              ))}
          </>
        )}
      </>
    );
  }

  const st = node.status ? node.status.toLowerCase() : "";
  return (
    <button
      className={`fp-row file ${node.status ? "changed s-" + st : ""}`}
      style={{ paddingLeft: pad + 13 }}
      onClick={() => onOpen(node.path)}
      onContextMenu={(e) => { e.preventDefault(); onMenu(node, e.clientX, e.clientY); }}
      title={node.path}
    >
      <FileIcon name={node.name} />
      {renaming ? (
        <NameInput initial={node.name} onCommit={onCommit} onCancel={onCancel} />
      ) : (
        <span className="fp-name">{node.name}</span>
      )}
      {node.status && !renaming && <span className={`fp-badge s-${st}`}>{node.status}</span>}
    </button>
  );
}

// ── inline name input (rename + create) ──────────────────────────────────
interface NameInputProps {
  initial: string;
  onCommit: (value: string) => void;
  onCancel: () => void;
}

function NameInput({ initial, onCommit, onCancel }: NameInputProps) {
  const ref = useRef<HTMLInputElement>(null);
  // Commit and cancel both unmount the input, which fires onBlur — guard so a
  // single edit resolves exactly once.
  const doneRef = useRef(false);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.focus();
    // Select the basename but not the extension, matching VS Code's rename.
    const dot = initial.lastIndexOf(".");
    if (dot > 0) el.setSelectionRange(0, dot);
    else el.select();
  }, [initial]);

  const commit = (v: string) => { if (!doneRef.current) { doneRef.current = true; onCommit(v); } };
  const cancel = () => { if (!doneRef.current) { doneRef.current = true; onCancel(); } };

  return (
    <input
      ref={ref}
      className="fp-name-input"
      defaultValue={initial}
      spellCheck={false}
      autoCapitalize="off"
      autoCorrect="off"
      onClick={(e) => { e.stopPropagation(); }}
      onMouseDown={(e) => { e.stopPropagation(); }}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") { e.preventDefault(); commit(e.currentTarget.value); }
        else if (e.key === "Escape") { e.preventDefault(); cancel(); }
      }}
      onBlur={(e) => commit(e.currentTarget.value)}
    />
  );
}

// A transient "new file / new folder" row holding just the name input.
interface CreateRowProps {
  mode: "newFile" | "newFolder";
  depth: number;
  onCommit: (value: string) => void;
  onCancel: () => void;
}

function CreateRow({ mode, depth, onCommit, onCancel }: CreateRowProps) {
  const pad = 10 + depth * 13;
  const isFolder = mode === "newFolder";
  return (
    <div className="fp-row file fp-creating" style={{ paddingLeft: pad + 13 }}>
      <FileIcon name="" folder={isFolder} open={isFolder} />
      <NameInput initial="" onCommit={onCommit} onCancel={onCancel} />
    </div>
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
  const name = basename(path);
  const dir = parentDir(path);

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

/** A non-colliding "… copy" path for Duplicate, e.g. `a/foo.ts` →
 *  `a/foo copy.ts`, then `a/foo copy 2.ts`, … against `existing`. */
export function duplicatePath(path: string, existing: Set<string>): string {
  const dir = parentDir(path);
  const base = basename(path);
  const dot = base.lastIndexOf(".");
  const stem = dot > 0 ? base.slice(0, dot) : base;
  const ext = dot > 0 ? base.slice(dot) : "";
  for (let n = 1; ; n++) {
    const suffix = n === 1 ? " copy" : ` copy ${n}`;
    const candidate = joinPath(dir, `${stem}${suffix}${ext}`);
    if (!existing.has(candidate)) return candidate;
  }
}

/** Best-effort message from a rejected Tauri command (errors serialize to a
 *  display string). */
function errMsg(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : "Operation failed";
}

/** Build a sorted nested tree (dirs first, then files; alpha within each)
 *  from a flat list of worktree files. `extraDirs` injects directories that
 *  carry no files yet (freshly-created empty folders). */
export function buildTree(files: WorktreeFile[], extraDirs: string[] = []): TreeNode[] {
  const roots: TreeNode[] = [];
  // path → DirNode, so we can attach children as we walk each file's segments.
  const dirIndex = new Map<string, DirNode>();

  const childrenOf = (path: string): TreeNode[] =>
    path === "" ? roots : (dirIndex.get(path) as DirNode).children;

  // Ensure a directory path (and its ancestors) exist as DirNodes.
  const ensureDir = (dir: string): void => {
    const segs = dir.split("/");
    let prefix = "";
    for (let i = 0; i < segs.length; i++) {
      const parent = prefix;
      prefix = prefix ? `${prefix}/${segs[i]}` : segs[i];
      if (!dirIndex.has(prefix)) {
        const node: DirNode = { type: "dir", name: segs[i], path: prefix, children: [] };
        dirIndex.set(prefix, node);
        childrenOf(parent).push(node);
      }
    }
  };

  for (const f of files) {
    const segs = f.path.split("/");
    if (segs.length > 1) ensureDir(segs.slice(0, -1).join("/"));
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

  for (const d of extraDirs) if (d) ensureDir(d);

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
