// One changed file's diff, opened from a changes list (the Code panel's or the
// Git panel's): the editor's header with a way back, the file's place in the
// list with prev/next (also `[` / `]`), and the diff, refetched whenever the
// file's revision moves so it tracks the agent's edits.
import type { DiffBaseMode, FileStatus } from "@/api";
import { Icon } from "@/components/Icon";
import { ViewerHeader } from "@/components/RightPanel/FilePanel/ViewerHeader";
import { IconButton } from "@/components/ui/IconButton";
import { useHljsTheme } from "@/util/codeTheme";
import { fileRevision, statusLetter } from "@/util/diff";
import { basename, parentDir } from "@/util/format";
import { useBareKeys } from "@/util/hooks";
import { DiffPane, extOf, useFileDiff } from "../DiffView";
import { useFreshLines } from "./useFreshLines";
import type { WalkthroughRange } from "./Walkthrough";

interface ChangedFileDiffProps {
  agentId: string;
  /** The list, in display order — drives the position and prev/next. */
  files: FileStatus[];
  path: string;
  base: DiffBaseMode;
  /** Prefixed to `path` for the diff fetch when the list's paths are relative
   *  to one repo of a multi-repo agent (the Git panel's are). */
  repoDir?: string;
  onSelect: (path: string) => void;
  onBack: () => void;
  /** Open the file in the editor. */
  onEdit?: (path: string) => void;
  viewed?: { on: boolean; toggle: () => void };
  /** The agent is writing this file now. */
  live?: boolean;
  focusRange?: WalkthroughRange;
}

export function ChangedFileDiff({
  agentId,
  files,
  path,
  base,
  repoDir,
  onSelect,
  onBack,
  onEdit,
  viewed,
  live,
  focusRange,
}: ChangedFileDiffProps) {
  const isBuiltInTheme = useHljsTheme();
  const index = files.findIndex((f) => f.path === path);
  const file = index >= 0 ? files[index] : undefined;
  const prev = index > 0 ? files[index - 1] : null;
  const next = index >= 0 && index < files.length - 1 ? files[index + 1] : null;

  const fetchPath = repoDir ? `${repoDir}/${path}` : path;
  // Refetched whenever the file's revision moves — an edit that keeps the
  // line counts included.
  const diff = useFileDiff(agentId, fetchPath, file ? fileRevision(file) : "", base);
  const freshKeys = useFreshLines(`${base}:${fetchPath}`, diff);

  useBareKeys({
    ...(prev && { "[": () => onSelect(prev.path) }),
    ...(next && { "]": () => onSelect(next.path) }),
  });

  return (
    <>
      <ViewerHeader
        name={basename(path)}
        dir={parentDir(path)}
        status={file ? statusLetter(file.kind) : null}
        dirty={false}
        onBack={onBack}
        backTitle="Back to changes"
        actions={
          <span className="fp-vh-actions iflex-center">
            {files.length > 1 && (
              <>
                <span className="cfd-pos text-xs">
                  {index + 1} / {files.length}
                </span>
                <IconButton
                  size="xs"
                  tip={prev ? `${basename(prev.path)} · [` : "First file"}
                  disabled={!prev}
                  onClick={() => prev && onSelect(prev.path)}
                >
                  <Icon name="chevL" />
                </IconButton>
                <IconButton
                  size="xs"
                  tip={next ? `${basename(next.path)} · ]` : "Last file"}
                  disabled={!next}
                  onClick={() => next && onSelect(next.path)}
                >
                  <Icon name="chevR" />
                </IconButton>
              </>
            )}
            {onEdit && (
              <button
                className="fp-meta-btn iflex-center tip"
                data-tip="Open this file in the editor"
                onClick={() => onEdit(path)}
              >
                <Icon name="edit" size={11} /> Edit
              </button>
            )}
            {viewed && (
              <button
                className={`fp-meta-btn iflex-center ${viewed.on ? "active" : ""}`}
                aria-pressed={viewed.on}
                onClick={viewed.toggle}
              >
                <Icon name="check" size={11} /> Viewed
              </button>
            )}
          </span>
        }
      />
      <DiffPane
        diff={diff}
        lang={extOf(path)}
        isBuiltInTheme={isBuiltInTheme}
        live={live}
        freshKeys={freshKeys}
        focusRange={focusRange}
      />
    </>
  );
}
