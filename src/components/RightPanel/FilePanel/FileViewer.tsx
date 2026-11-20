// Loads a checkout file's contents and routes to the right view: the image
// itself for an image, a "no preview" message for other binary / too-large /
// unreadable files, a loading state, or the editable FileEditor.
import type { AgentRecord, DiffBaseMode } from "@/api";
import { ImagePreview } from "@/components/RightPanel/ImagePreview";
import { basename, parentDir } from "@/util/format";
import { FileEditor } from "./FileEditor";
import { useCheckoutFile } from "./useCheckoutFile";
import { ViewerHeader } from "./ViewerHeader";

interface FileViewerProps {
  agent: AgentRecord;
  path: string;
  diffBase: DiffBaseMode;
  onBack: () => void;
}

export function FileViewer({ agent, path, diffBase, onBack }: FileViewerProps) {
  const name = basename(path);
  const dir = parentDir(path);
  const { contents, error } = useCheckoutFile(agent.id, path, diffBase);

  // A raster image has no text to edit. An SVG does, and goes to the editor,
  // which offers the preview.
  if (contents?.binary && contents.image) {
    return (
      <div className="fp-wrap">
        <ViewerHeader
          name={name}
          dir={dir}
          onBack={onBack}
          status={contents.status}
          dirty={false}
        />
        <ImagePreview image={contents.image} name={name} />
      </div>
    );
  }

  if (error || (contents && (contents.binary || contents.too_large))) {
    return (
      <div className="fp-wrap">
        <ViewerHeader
          name={name}
          dir={dir}
          onBack={onBack}
          status={contents?.status ?? null}
          dirty={false}
        />
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
      diffBase={diffBase}
      onBack={onBack}
    />
  );
}
