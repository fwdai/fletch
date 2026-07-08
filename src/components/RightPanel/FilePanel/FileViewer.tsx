// Loads a checkout file's contents and routes to the right view: a "no
// preview" message for binary / too-large / unreadable files, a loading
// state, or the editable FileEditor.
import { useEffect, useState } from "react";
import { type AgentRecord, api, type CheckoutFileContents } from "@/api";
import { basename, parentDir } from "@/util/format";
import { FileEditor } from "./FileEditor";
import { ViewerHeader } from "./ViewerHeader";

interface FileViewerProps {
  agent: AgentRecord;
  path: string;
  onBack: () => void;
}

export function FileViewer({ agent, path, onBack }: FileViewerProps) {
  const name = basename(path);
  const dir = parentDir(path);

  const [contents, setContents] = useState<CheckoutFileContents | null>(null);
  const [error, setError] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setContents(null);
    setError(false);
    api
      .readCheckoutFile(agent.id, path)
      .then((c) => {
        if (!cancelled) setContents(c);
      })
      .catch(() => {
        if (!cancelled) setError(true);
      });
    return () => {
      cancelled = true;
    };
  }, [agent.id, path]);

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
      onBack={onBack}
    />
  );
}
