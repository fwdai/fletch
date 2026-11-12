import { Icon } from "@/components/Icon";
import { SetRow } from "@/components/SettingsScreen/primitives";
import { IconButton } from "@/components/ui/IconButton";
import { FILE_LABEL, fileHolding } from "../projectFile/files";
import type { ProjectFileEditor } from "../projectFile/useProjectFileEditor";

/** The folders a starting disk never carries over, one compact row each with
 *  the file that holds it and a way to take it out of that file. */
export function ExcludedPaths({ editor, paths }: { editor: ProjectFileEditor; paths: string[] }) {
  const { files, canEdit, edit } = editor;
  return (
    <>
      {paths.map((path) => {
        const file = fileHolding(files, (f) => f.seed.exclude.includes(path));
        return (
          <SetRow
            key={path}
            compact
            title={<span className="mono">{path}</span>}
            sub={file ? FILE_LABEL[file] : undefined}
          >
            {canEdit && file && (
              <IconButton
                size="xs"
                danger
                tip="Carry this folder over again"
                aria-label={`Remove ${path}`}
                onClick={() => void edit(file, { seedExcludeRemove: [path] })}
              >
                <Icon name="close" size={11} />
              </IconButton>
            )}
          </SetRow>
        );
      })}
    </>
  );
}
