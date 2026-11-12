import { Icon } from "@/components/Icon";
import { SetRow } from "@/components/SettingsScreen/primitives";
import { IconButton } from "@/components/ui/IconButton";
import { FILE_LABEL, FILES, fileReport } from "../projectFile/files";
import type { ProjectFileEditor } from "../projectFile/useProjectFileEditor";

/** The folders a starting disk never carries over: one compact row for each
 *  entry of each file, with a way to take it out of that file. A folder both
 *  files leave out has two rows, since it stays out until both entries go. */
export function ExcludedPaths({ editor }: { editor: ProjectFileEditor }) {
  const { files, canEdit, edit } = editor;
  return (
    <>
      {FILES.flatMap((file) =>
        (fileReport(files, file)?.seed.exclude ?? []).map((path) => (
          <SetRow
            key={`${file}:${path}`}
            compact
            title={<span className="mono">{path}</span>}
            sub={FILE_LABEL[file]}
          >
            {canEdit && (
              <IconButton
                size="xs"
                danger
                tip="Take this entry out of the file"
                aria-label={`Remove ${path} from ${FILE_LABEL[file]}`}
                onClick={() => void edit(file, { seedExcludeRemove: [path] })}
              >
                <Icon name="close" size={11} />
              </IconButton>
            )}
          </SetRow>
        )),
      )}
    </>
  );
}
