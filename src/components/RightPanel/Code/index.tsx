// CodePanel — the unified "Code" right-rail tab. It hosts two modes behind a
// secondary in-panel switch:
//   • Files — browse & edit the worktree (the existing <FilePanel>).
//   • Live  — an activity feed of the agent's edits as diffs (<CodeLivePanel>).
//
// The open/selected file is owned here so it survives a mode switch and so the
// cross-links work: the editor's "Diff" button jumps to Live on that file, and
// Live's "Edit" button opens the file back in Files mode.
import { useEffect, useState } from "react";
import type { AgentRecord } from "@/api";
import { Icon } from "@/components/Icon";
import { FilePanel } from "@/components/RightPanel/FilePanel";
import { useAppStore } from "@/store";
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
  useEffect(() => {
    setSelectedPath(null);
  }, [agent.id]);

  const changeMode = (m: Mode) => {
    setMode(m);
    localStorage.setItem(MODE_KEY, m);
  };

  return (
    <div className="code-panel">
      <ModeSwitch agent={agent} mode={mode} onChange={changeMode} />
      <div className="code-panel-body">
        {mode === "files" ? (
          <FilePanel agent={agent} openPath={selectedPath} onOpenPath={setSelectedPath} />
        ) : (
          <CodeLivePanel
            agent={agent}
            selectedPath={selectedPath}
            onSelect={setSelectedPath}
            onOpenInEditor={(p) => {
              setSelectedPath(p);
              changeMode("files");
            }}
          />
        )}
      </div>
    </div>
  );
}

// A secondary segmented control — deliberately styled unlike the panel tabs
// above it (filled "thumb" pill, not an underline tab) so it reads as a control
// within the Code panel, not as panel switching. The two modes are the two
// ways to look at code here: explore it yourself, or watch the agent change it.
function ModeSwitch({
  agent,
  mode,
  onChange,
}: {
  agent: AgentRecord;
  mode: Mode;
  onChange: (m: Mode) => void;
}) {
  const hasChanges = useAppStore(
    (s) => (s.gitStates[agent.id]?.files.length ?? s.gitShortstats[agent.id]?.file_count ?? 0) > 0,
  );
  // Whether the agent is mid-turn — the dot is green & pulsing only then, and
  // goes grey when work stops so it never implies activity that isn't there.
  const busy = useAppStore((s) => s.managedBusy[agent.id] ?? false);

  return (
    <div className="code-modes flex-center">
      <div className="code-modeswitch" role="tablist" aria-label="Code view mode">
        <button
          role="tab"
          aria-selected={mode === "files"}
          className={`cms-seg iflex-center text-xs ${mode === "files" ? "active" : ""} tip`}
          data-tip-down
          data-tip="Browse and edit any file in the worktree"
          onClick={() => onChange("files")}
        >
          <Icon name="folder" size={12} />
          <span>Explore</span>
        </button>
        <button
          role="tab"
          aria-selected={mode === "live"}
          className={`cms-seg iflex-center text-xs ${mode === "live" ? "active" : ""} tip`}
          data-tip-down
          data-tip="Watch the agent's changes as they happen"
          onClick={() => onChange("live")}
        >
          <Icon name="zap" size={12} />
          <span>Live</span>
          {(hasChanges || busy) && <span className={`cms-live-dot ${busy ? "on" : ""}`}></span>}
        </button>
      </div>
    </div>
  );
}
