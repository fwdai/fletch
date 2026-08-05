// CodePanel — the unified "Code" right-rail tab. It hosts two modes behind a
// secondary in-panel switch:
//   • Files — browse & edit the checkout (the existing <FilePanel>).
//   • Live  — an activity feed of the agent's edits as diffs (<CodeLivePanel>).
//
// The open/selected file is owned here so it survives a mode switch and so the
// cross-links work: the editor's "Diff" button jumps to Live on that file, and
// Live's "Edit" button opens the file back in Files mode.
import { useEffect, useState } from "react";
import type { AgentRecord, DiffBaseMode } from "@/api";
import { Icon } from "@/components/Icon";
import { FilePanel } from "@/components/RightPanel/FilePanel";
import { useAppStore } from "@/store";
import { CodeLivePanel } from "./CodeLivePanel";

type Mode = "files" | "live";
const MODE_KEY = "q2:codeMode";
const BASE_KEY = "q2:diffBase";

function loadMode(): Mode {
  return localStorage.getItem(MODE_KEY) === "live" ? "live" : "files";
}

function loadBase(): DiffBaseMode {
  return localStorage.getItem(BASE_KEY) === "head" ? "head" : "fork";
}

export function CodePanel({ agent }: { agent: AgentRecord }) {
  const [mode, setMode] = useState<Mode>(loadMode);
  const [diffBase, setDiffBase] = useState<DiffBaseMode>(loadBase);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);

  // The selected file is per-agent; drop it when the agent changes.
  useEffect(() => {
    setSelectedPath(null);
  }, [agent.id]);

  const changeMode = (m: Mode) => {
    setMode(m);
    localStorage.setItem(MODE_KEY, m);
  };

  const changeBase = (b: DiffBaseMode) => {
    setDiffBase(b);
    localStorage.setItem(BASE_KEY, b);
  };

  return (
    <div className="code-panel">
      <div className="code-modes">
        <ModeSwitch agent={agent} mode={mode} onChange={changeMode} />
        <BaseSwitch value={diffBase} onChange={changeBase} />
      </div>
      <div className="code-panel-body">
        {mode === "files" ? (
          <FilePanel
            agent={agent}
            openPath={selectedPath}
            onOpenPath={setSelectedPath}
            diffBase={diffBase}
          />
        ) : (
          <CodeLivePanel
            agent={agent}
            selectedPath={selectedPath}
            onSelect={setSelectedPath}
            diffBase={diffBase}
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

// What the diffs measure against: the workspace's starting commit (everything
// the agent has done here) or the latest commit (only uncommitted work). The
// file lists always count vs the latest commit, so "Uncommitted" is the mode
// where the diff matches the +/− counts beside it.
function BaseSwitch({
  value,
  onChange,
}: {
  value: DiffBaseMode;
  onChange: (b: DiffBaseMode) => void;
}) {
  return (
    <div className="code-modeswitch" role="tablist" aria-label="Diff base">
      <button
        role="tab"
        aria-selected={value === "fork"}
        className={`cms-seg iflex-center text-xs ${value === "fork" ? "active" : ""} tip`}
        data-tip-down
        data-tip="Diff everything changed in this workspace, since it started"
        onClick={() => onChange("fork")}
      >
        <span>All</span>
      </button>
      <button
        role="tab"
        aria-selected={value === "head"}
        className={`cms-seg iflex-center text-xs ${value === "head" ? "active" : ""} tip`}
        data-tip-down
        data-tip="Diff only uncommitted work, since the last commit"
        onClick={() => onChange("head")}
      >
        <span>Uncommitted</span>
      </button>
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
    <div className="code-modeswitch" role="tablist" aria-label="Code view mode">
      <button
        role="tab"
        aria-selected={mode === "files"}
        className={`cms-seg iflex-center text-xs ${mode === "files" ? "active" : ""} tip`}
        data-tip-down
        data-tip="Browse and edit any file in the checkout"
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
  );
}
