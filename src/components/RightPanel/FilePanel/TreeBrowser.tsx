// The Files-panel explorer: search + "Changed" filter toolbar, an inline
// error banner, and a VS Code-style recursive tree. Presentational — all
// state and file operations live in the FilePanel orchestrator (so they
// survive opening/closing the editor) and arrive here as props.
import { useEffect, useRef } from "react";
import { Icon } from "@/components/Icon";
import { FileIcon } from "@/components/RightPanel/FileIcon";
import { IconButton } from "@/components/ui/IconButton";
import type { EditState, TreeNode } from "./tree";

interface TreeBrowserProps {
  tree: TreeNode[];
  loaded: boolean;
  filtering: boolean;
  query: string;
  onQueryChange: (q: string) => void;
  changedOnly: boolean;
  changedCount: number;
  onToggleChangedOnly: () => void;
  onCollapseAll: () => void;
  onBeginCreate: (mode: "newFile" | "newFolder", dir: string) => void;
  expanded: Set<string>;
  onToggleDir: (path: string) => void;
  edit: EditState | null;
  onCommit: (value: string) => void;
  onCancel: () => void;
  onOpen: (path: string) => void;
  onMenu: (node: TreeNode, x: number, y: number) => void;
  opError: string | null;
  onClearOpError: () => void;
}

export function TreeBrowser({
  tree,
  loaded,
  filtering,
  query,
  onQueryChange,
  changedOnly,
  changedCount,
  onToggleChangedOnly,
  onCollapseAll,
  onBeginCreate,
  expanded,
  onToggleDir,
  edit,
  onCommit,
  onCancel,
  onOpen,
  onMenu,
  opError,
  onClearOpError,
}: TreeBrowserProps) {
  return (
    <div className="fp-wrap">
      <div className="fp-toolbar flex-center">
        <div className="fp-search flex-center">
          <Icon name="search" size={12} />
          <input
            type="text"
            placeholder="Search files"
            value={query}
            onChange={(e) => onQueryChange(e.target.value)}
            spellCheck={false}
          />
          {query && (
            <button
              className="fp-clear iflex-center"
              onClick={() => onQueryChange("")}
              aria-label="Clear"
            >
              <Icon name="close" size={10} />
            </button>
          )}
        </div>
        <button
          className={`fp-filter iflex-center text-xs ${changedOnly ? "on" : ""}`}
          title={changedOnly ? "Showing changed files only" : "Show only files the agent changed"}
          onClick={onToggleChangedOnly}
        >
          <span className="fp-filter-dot"></span>
          Changed
          <span className="fp-filter-count text-xs">{changedCount}</span>
        </button>
        <IconButton size="xs" tip="New file" tipDown onClick={() => onBeginCreate("newFile", "")}>
          <Icon name="file" />
        </IconButton>
        <IconButton
          size="xs"
          tip="New folder"
          tipDown
          onClick={() => onBeginCreate("newFolder", "")}
        >
          <Icon name="folder" />
        </IconButton>
        <IconButton size="xs" tip="Collapse all" tipDown onClick={onCollapseAll}>
          <Icon name="shrink" />
        </IconButton>
      </div>

      {opError && (
        // Key by message so a new error remounts the banner and re-runs the
        // attention flash even when one is already showing.
        <div key={opError} className="fp-op-error flex-center text-sm" role="alert">
          <Icon name="close" size={11} />
          <span>{opError}</span>
          <button className="fp-clear iflex-center" onClick={onClearOpError} aria-label="Dismiss">
            <Icon name="close" size={10} />
          </button>
        </div>
      )}

      <div className="fp-tree">
        {edit && edit.mode !== "rename" && edit.parentDir === "" && (
          <CreateRow mode={edit.mode} depth={0} onCommit={onCommit} onCancel={onCancel} />
        )}
        {tree.length === 0 && !edit ? (
          <div className="empty-msg" style={{ margin: "auto" }}>
            <div className="et">{loaded ? "No matching files" : "Loading…"}</div>
            {loaded && (
              <div>{changedOnly ? "Clear the Changed filter or " : ""}adjust your search.</div>
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
              onToggle={onToggleDir}
              onOpen={onOpen}
              onMenu={onMenu}
              onCommit={onCommit}
              onCancel={onCancel}
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
  edit: EditState | null;
  onToggle: (path: string) => void;
  onOpen: (path: string) => void;
  onMenu: (node: TreeNode, x: number, y: number) => void;
  onCommit: (value: string) => void;
  onCancel: () => void;
}

function TreeRow({
  node,
  depth,
  expanded,
  forceOpen,
  edit,
  onToggle,
  onOpen,
  onMenu,
  onCommit,
  onCancel,
}: TreeRowProps) {
  const pad = 10 + depth * 13;
  const renaming = edit?.mode === "rename" && edit.path === node.path;

  if (node.type === "dir") {
    const isOpen = forceOpen || expanded.has(node.path);
    const creatingHere = edit && edit.mode !== "rename" && edit.parentDir === node.path;
    return (
      <>
        <button
          className="fp-row flex-center dir"
          style={{ paddingLeft: pad }}
          onClick={() => onToggle(node.path)}
          onContextMenu={(e) => {
            e.preventDefault();
            onMenu(node, e.clientX, e.clientY);
          }}
        >
          <span className={`fp-twisty iflex-center ${isOpen ? "open" : ""}`}>
            <Icon name="chevR" size={11} />
          </span>
          <FileIcon name={node.name} folder open={isOpen} />
          {renaming ? (
            <NameInput initial={node.name} onCommit={onCommit} onCancel={onCancel} />
          ) : (
            <span className="fp-name truncate">{node.name}</span>
          )}
          {/* When collapsed, surface that this folder hides agent edits. */}
          {!isOpen && !renaming && node.changedCount > 0 && (
            <span
              className="fp-dir-changed"
              title={`${node.changedCount} changed file${node.changedCount === 1 ? "" : "s"} inside`}
            >
              {node.changedCount}
            </span>
          )}
        </button>
        {(isOpen || creatingHere) && (
          <>
            {creatingHere && edit && (
              <CreateRow
                mode={edit.mode}
                depth={depth + 1}
                onCommit={onCommit}
                onCancel={onCancel}
              />
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
      className={`fp-row flex-center file ${node.status ? `changed s-${st}` : ""}`}
      style={{ paddingLeft: pad + 13 }}
      onClick={() => onOpen(node.path)}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu(node, e.clientX, e.clientY);
      }}
      title={node.path}
    >
      <FileIcon name={node.name} />
      {renaming ? (
        <NameInput initial={node.name} onCommit={onCommit} onCancel={onCancel} />
      ) : (
        <span className="fp-name truncate">{node.name}</span>
      )}
      {node.status && !renaming && (
        <span className={`fp-badge iflex-center s-${st}`}>{node.status}</span>
      )}
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

  const commit = (v: string) => {
    if (!doneRef.current) {
      doneRef.current = true;
      onCommit(v);
    }
  };
  const cancel = () => {
    if (!doneRef.current) {
      doneRef.current = true;
      onCancel();
    }
  };

  return (
    <input
      ref={ref}
      className="fp-name-input"
      defaultValue={initial}
      spellCheck={false}
      autoCapitalize="off"
      autoCorrect="off"
      onClick={(e) => {
        e.stopPropagation();
      }}
      onMouseDown={(e) => {
        e.stopPropagation();
      }}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") {
          e.preventDefault();
          commit(e.currentTarget.value);
        } else if (e.key === "Escape") {
          e.preventDefault();
          cancel();
        }
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
    <div className="fp-row flex-center file fp-creating" style={{ paddingLeft: pad + 13 }}>
      <FileIcon name="" folder={isFolder} open={isFolder} />
      <NameInput initial="" onCommit={onCommit} onCancel={onCancel} />
    </div>
  );
}
