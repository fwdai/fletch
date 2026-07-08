// FilePanel — the "Files" mode of the Code panel. Browse the agent's checkout
// and view/edit any file's contents (diffs live in the Code panel's Live mode).
//
// This orchestrator owns the explorer's state + file operations and switches
// between two modes:
//   • Explorer — <TreeBrowser>: a VS Code-style tree with a "Changed" filter
//     and search. Agent-touched files carry a git status + colored name.
//   • Editor   — <FileViewer> → <FileEditor>: a transparent <textarea> over a
//     live syntax-highlight layer, with line numbers and a change gutter.
//
// Explorer state lives here (not in TreeBrowser) so it survives a round-trip
// through the editor — notably `pendingDirs`, which tracks freshly-created
// empty folders that git wouldn't otherwise list.
//
// Faithful port of the design (fletch v2 files.jsx), wired to the real
// checkout via the `*_checkout_*` Tauri commands.
import { useCallback, useEffect, useMemo, useState } from "react";
import { type AgentRecord, api, type CheckoutFile } from "@/api";
import { type ContextMenuEntry, FileContextMenu } from "@/components/RightPanel/FileContextMenu";
import { joinPath, parentDir } from "@/util/format";
import { usePoll } from "@/util/hooks";
import { FileViewer } from "./FileViewer";
import { TreeBrowser } from "./TreeBrowser";
import {
  buildTree,
  duplicatePath,
  type EditState,
  errMsg,
  type FileNode,
  filterTree,
  type MenuState,
  type TreeNode,
} from "./tree";

interface FilePanelProps {
  agent: AgentRecord;
  // The open file is owned by the parent Code panel so it survives a switch to
  // Live mode (and so "Open in editor" from Live can target it).
  openPath: string | null;
  onOpenPath: (path: string | null) => void;
}

export function FilePanel({ agent, openPath, onOpenPath }: FilePanelProps) {
  const [files, setFiles] = useState<CheckoutFile[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [query, setQuery] = useState("");
  const [changedOnly, setChangedOnly] = useState(false);
  const [expanded, setExpanded] = useState<Set<string>>(() => new Set());
  const [menu, setMenu] = useState<MenuState | null>(null);
  const [edit, setEdit] = useState<EditState | null>(null);
  const [opError, setOpError] = useState<string | null>(null);
  // Newly-created directories that are still empty. git lists files, not dirs,
  // so without this a brand-new folder would vanish on the next poll.
  const [pendingDirs, setPendingDirs] = useState<Set<string>>(() => new Set());

  // Reset per-agent so one checkout's search/expansion doesn't leak. (The open
  // file is owned by the parent and reset there.)
  useEffect(() => {
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
      setFiles(await api.listCheckoutTree(agent.id));
    } catch {
      // Keep the previous tree on a transient IPC error rather than blanking it.
    }
    setLoaded(true);
  }, [agent.id]);

  // Poll the tree at 2s, but only while the explorer is showing — no point
  // re-listing the checkout while a file is open in the editor.
  const pollTree = useCallback(async () => {
    if (!openPath) await refresh();
  }, [openPath, refresh]);
  usePoll(pollTree, 2000, [pollTree]);

  const fullTree = useMemo(() => buildTree(files, [...pendingDirs]), [files, pendingDirs]);

  // Once a real file lands inside a tracked empty-dir, git lists the dir via
  // that file, so we can stop injecting it (and keep the set from growing).
  useEffect(() => {
    setPendingDirs((s) => {
      if (!s.size) return s;
      const next = new Set(
        [...s].filter(
          (d) => !files.some((file) => file.path === d || file.path.startsWith(`${d}/`)),
        ),
      );
      return next.size === s.size ? s : next;
    });
  }, [files]);

  // The tree starts fully collapsed so the user expands only what they need.
  // (The per-agent reset above already clears `expanded` when switching agents.)

  const changedCount = useMemo(() => files.filter((f) => f.status).length, [files]);
  const filtering = query.trim() !== "" || changedOnly;
  const tree = filtering ? filterTree(fullTree, query.trim().toLowerCase(), changedOnly) : fullTree;

  const toggleDir = (path: string) =>
    setExpanded((s) => {
      const n = new Set(s);
      if (n.has(path)) n.delete(path);
      else n.add(path);
      return n;
    });

  const expand = (path: string) => setExpanded((s) => new Set(s).add(path));

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
    if (!name) {
      setEdit(null);
      return;
    }
    setOpError(null);
    try {
      if (edit.mode === "rename") {
        const from = edit.path;
        const dest = joinPath(parentDir(from), name);
        if (dest === from) {
          setEdit(null);
          return;
        }
        await api.renameCheckoutPath(agent.id, from, dest);
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
        await api.createCheckoutFile(agent.id, dest);
        setEdit(null);
        await refresh();
        onOpenPath(dest);
        return;
      } else {
        const dest = joinPath(edit.parentDir, name);
        await api.createCheckoutDir(agent.id, dest);
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
      await api.deleteCheckoutPath(agent.id, node.path);
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
      await api.copyCheckoutFile(agent.id, node.path, dest);
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
      {
        icon: "edit",
        label: "Rename…",
        onClick: () => {
          setOpError(null);
          setEdit({ mode: "rename", path: node.path, isDir: node.type === "dir" });
        },
      },
      { icon: "copy", label: "Copy Path", feedbackLabel: "Copied", onClick: () => copyPath(node) },
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

  // ── editor mode ───────────────────────────────────────────────────────
  if (openPath) {
    return (
      <FileViewer
        key={openPath}
        agent={agent}
        path={openPath}
        onBack={() => {
          onOpenPath(null);
          void refresh();
        }}
      />
    );
  }

  // ── explorer mode ─────────────────────────────────────────────────────
  return (
    <>
      <TreeBrowser
        tree={tree}
        loaded={loaded}
        filtering={filtering}
        query={query}
        onQueryChange={setQuery}
        changedOnly={changedOnly}
        changedCount={changedCount}
        onToggleChangedOnly={() => setChangedOnly((v) => !v)}
        onCollapseAll={() => setExpanded(new Set())}
        onBeginCreate={beginCreate}
        expanded={expanded}
        onToggleDir={toggleDir}
        edit={edit}
        onCommit={commitEdit}
        onCancel={cancelEdit}
        onOpen={onOpenPath}
        onMenu={(node, x, y) => setMenu({ node, x, y })}
        opError={opError}
        onClearOpError={() => setOpError(null)}
      />
      {menu && (
        <FileContextMenu
          x={menu.x}
          y={menu.y}
          entries={menuEntries(menu.node)}
          onClose={() => setMenu(null)}
        />
      )}
    </>
  );
}
