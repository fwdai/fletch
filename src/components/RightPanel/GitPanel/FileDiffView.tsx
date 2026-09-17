// One changed file's uncommitted diff, opened from the Git panel's changes
// list — the desktop counterpart of the phone's Diff screen. Composes the
// Code panel's header and diff primitives; prev/next step through the list.
import type { FileStatus } from "@/api";
import { Icon } from "@/components/Icon";
import { extOf, FileDiff } from "@/components/RightPanel/Code/DiffView";
import { ViewerHeader } from "@/components/RightPanel/FilePanel/ViewerHeader";
import { IconButton } from "@/components/ui/IconButton";
import { useHljsTheme } from "@/util/codeTheme";
import { statusLetter } from "@/util/diff";
import { basename, parentDir } from "@/util/format";

export function FileDiffView({
  agentId,
  files,
  path,
  onSelect,
  onBack,
}: {
  agentId: string;
  /** The changes list, in display order — drives prev/next. */
  files: FileStatus[];
  path: string;
  onSelect: (path: string) => void;
  onBack: () => void;
}) {
  const isBuiltInTheme = useHljsTheme();
  const index = files.findIndex((f) => f.path === path);
  const file = files[index];
  const prev = index > 0 ? files[index - 1] : null;
  const next = index >= 0 && index < files.length - 1 ? files[index + 1] : null;

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
          files.length > 1 && (
            <span className="fp-vh-actions iflex-center">
              <span className="git-diff-pos text-xs">
                {index + 1} / {files.length}
              </span>
              <IconButton
                size="xs"
                tip={prev ? basename(prev.path) : "First file"}
                disabled={!prev}
                onClick={() => prev && onSelect(prev.path)}
              >
                <Icon name="chevL" />
              </IconButton>
              <IconButton
                size="xs"
                tip={next ? basename(next.path) : "Last file"}
                disabled={!next}
                onClick={() => next && onSelect(next.path)}
              >
                <Icon name="chevR" />
              </IconButton>
            </span>
          )
        }
      />
      {/* The list's +/− counts are vs the latest commit, so the diff is too. */}
      <FileDiff
        agentId={agentId}
        path={path}
        lang={extOf(path)}
        isBuiltInTheme={isBuiltInTheme}
        base="head"
      />
    </>
  );
}
