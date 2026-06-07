// CodePanel — the unified "Code" right-rail tab. It hosts two modes behind a
// secondary in-panel switch:
//   • Files — browse & edit the worktree (the existing <FilePanel>).
//   • Live  — an activity feed of the agent's edits as diffs (<CodeLivePanel>).
//
// The open/selected file is owned here so it survives a mode switch and so the
// cross-links work: the editor's "Diff" button jumps to Live on that file, and
// Live's "Edit" button opens the file back in Files mode.
import { useEffect, useState } from "react";
import type { AgentRecord } from "../../../api";
import { useAppStore } from "../../../store";
import { Icon } from "../../Icon";
import { FilePanel } from "../FilePanel";
import { CodeLivePanel } from "./CodeLivePanel";

type Mode = "files" | "live";
const MODE_KEY = "q2:codeMode";

function loadMode(): Mode {
  return localStorage.getItem(MODE_KEY) === "live" ? "live" : "files";
}

export function CodePanel({ agent }: { agent: AgentRecord }) {
  const [mode, setMode] = useState<Mode>(loadMode);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);

  // The selected file is per-agent; drop it when the agent changes.
  useEffect(() => { setSelectedPath(null); }, [agent.id]);

  const changeMode = (m: Mode) => {
    setMode(m);
    localStorage.setItem(MODE_KEY, m);
  };

  return (
    <div className="code-panel">
      <ModeSwitch agent={agent} mode={mode} onChange={changeMode} />
      <div className="code-panel-body">
        {mode === "files" ? (
          <FilePanel
            agent={agent}
            openPath={selectedPath}
            onOpenPath={setSelectedPath}
            canViewDiff
            onViewDiff={() => changeMode("live")}
          />
        ) : (
          <CodeLivePanel
            agent={agent}
            selectedPath={selectedPath}
            onSelect={setSelectedPath}
            onOpenInEditor={(p) => { setSelectedPath(p); changeMode("files"); }}
          />
        )}
      </div>
    </div>
  );
}

// A secondary segmented control — deliberately styled unlike the panel tabs
// above it (filled "thumb" pill, not an underline tab) so it reads as a control
// within the Code panel, not as panel switching.
function ModeSwitch({ agent, mode, onChange }: { agent: AgentRecord; mode: Mode; onChange: (m: Mode) => void }) {
  // A live dot on the Live segment when the worktree has uncommitted changes,
  // so the user can tell there's something to watch without leaving Files mode.
  const hasChanges = useAppStore(
    (s) =>
      (s.gitStates[agent.id]?.files.length ??
        s.gitShortstats[agent.id]?.file_count ??
        0) > 0,
  );

  return (
    <div className="code-modes">
      <div className="code-modeswitch" role="tablist" aria-label="Code view mode">
        <button
          role="tab"
          aria-selected={mode === "files"}
          className={`cms-seg ${mode === "files" ? "active" : ""}`}
          onClick={() => onChange("files")}
        >
          <Icon name="folder" size={12} />
          <span>Files</span>
        </button>
        <button
          role="tab"
          aria-selected={mode === "live"}
          className={`cms-seg ${mode === "live" ? "active" : ""}`}
          onClick={() => onChange("live")}
        >
          <Icon name="zap" size={12} />
          <span>Live</span>
          {hasChanges && <span className="cms-live-dot"></span>}
        </button>
      </div>
    </div>
  );
}
