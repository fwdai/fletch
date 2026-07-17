// ReviewSurface/DiffPanel — the ferried diff: a file list on the left, the
// selected file's unified diff on the right. Reuses the Code panel's diff
// machinery (`parseUnifiedDiff` + `DiffBody`) so highlighting and markup match
// the rest of the app; the diff source is injected (`getDiff`) so the surface
// stays decoupled from any run-specific fetch.

import { useEffect, useState } from "react";
import type { GateDiffFile } from "../../api";
import { useHljsTheme } from "../../util/codeTheme";
import { type DiffHunk, parseUnifiedDiff } from "../../util/diff";
import { Icon } from "../Icon";
import { DiffBody, extOf } from "../RightPanel/Code/DiffView";

export function DiffPanel({
  files,
  getDiff,
}: {
  files: GateDiffFile[];
  getDiff: (path: string | null) => Promise<string>;
}) {
  const [selected, setSelected] = useState<string | null>(files[0]?.path ?? null);
  const isBuiltInTheme = useHljsTheme();

  // Keep the selection valid as the file set changes (e.g. a re-review).
  useEffect(() => {
    if (files.length > 0 && !files.some((f) => f.path === selected)) {
      setSelected(files[0].path);
    }
  }, [files, selected]);

  if (files.length === 0) {
    return (
      <div className="empty-msg" style={{ margin: "auto" }}>
        <div className="et">No file changes</div>
        <div>This step produced no diff against the run base.</div>
      </div>
    );
  }

  return (
    <div className="rv-diff">
      <div className="rv-files">
        {files.map((f) => (
          <button
            key={f.path}
            type="button"
            className={`rv-file ${f.path === selected ? "active" : ""}`}
            onClick={() => setSelected(f.path)}
          >
            <Icon name="file" size={12} className="rv-file-icon" />
            <span className="rv-file-path truncate">{f.path}</span>
            <span className="rv-file-stat">
              <span className="add">+{f.additions}</span>
              <span className="rem">−{f.deletions}</span>
            </span>
          </button>
        ))}
      </div>
      <div className="rv-diff-body">
        {selected && (
          <FileDiff
            key={selected}
            path={selected}
            getDiff={getDiff}
            isBuiltInTheme={isBuiltInTheme}
          />
        )}
      </div>
    </div>
  );
}

type LoadState = "loading" | "loaded" | "error";

function FileDiff({
  path,
  getDiff,
  isBuiltInTheme,
}: {
  path: string;
  getDiff: (path: string | null) => Promise<string>;
  isBuiltInTheme: boolean;
}) {
  const [hunks, setHunks] = useState<DiffHunk[]>([]);
  const [state, setState] = useState<LoadState>("loading");

  useEffect(() => {
    let cancelled = false;
    setState("loading");
    getDiff(path)
      .then((text) => {
        if (cancelled) return;
        setHunks(parseUnifiedDiff(text));
        setState("loaded");
      })
      .catch(() => {
        if (!cancelled) setState("error");
      });
    return () => {
      cancelled = true;
    };
  }, [path, getDiff]);

  if (state === "loading") {
    return (
      <div className="empty-msg" style={{ margin: "auto" }}>
        <div className="et">Loading diff…</div>
      </div>
    );
  }
  if (state === "error") {
    return (
      <div className="empty-msg" style={{ margin: "auto" }}>
        <div className="et">Couldn't load diff</div>
        <div>Reopen the review to try again.</div>
      </div>
    );
  }
  if (hunks.length === 0) {
    return (
      <div className="empty-msg" style={{ margin: "auto" }}>
        <div className="et">No textual diff</div>
        <div>This change has no line-level diff to show.</div>
      </div>
    );
  }
  return <DiffBody hunks={hunks} lang={extOf(path)} isBuiltInTheme={isBuiltInTheme} />;
}
