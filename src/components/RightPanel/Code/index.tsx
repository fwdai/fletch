// CodePanel — the unified "Code" right-rail tab. It hosts two modes behind a
// secondary in-panel switch:
//   • Changes — every file this workspace changed, each opening its diff
//     (<Changes>), with Follow to track the agent during a turn.
//   • Explore — browse & edit the checkout (<FilePanel>).
//
// The open/selected file is owned here so it survives a mode switch and so the
// cross-link works: a diff's "Edit" button opens the file in Explore. The scope
// switch (All / Uncommitted) sits beside the modes because both use it — the
// Changes list and diffs, and the editor's gutter and Diff toggle.
import { useEffect, useState } from "react";
import type { AgentRecord, DiffBaseMode } from "@/api";
import { Icon } from "@/components/Icon";
import { FilePanel } from "@/components/RightPanel/FilePanel";
import { isAgentBusy } from "@/helpers";
import { useAppStore } from "@/store";
import { Changes } from "./Changes";
import { type CodeMode, loadBase, loadMode, saveBase, saveMode } from "./prefs";

export function CodePanel({ agent }: { agent: AgentRecord }) {
  const [mode, setMode] = useState<CodeMode>(loadMode);
  const [diffBase, setDiffBase] = useState<DiffBaseMode>(loadBase);
  const [selectedPath, setSelectedPath] = useState<string | null>(null);

  // The selected file is per-agent; drop it when the agent changes.
  // biome-ignore lint/correctness/useExhaustiveDependencies: agent.id is the intended reset trigger
  useEffect(() => {
    setSelectedPath(null);
  }, [agent.id]);

  const changeMode = (m: CodeMode) => {
    setMode(m);
    saveMode(m);
  };

  const changeBase = (b: DiffBaseMode) => {
    setDiffBase(b);
    saveBase(b);
  };

  return (
    <div className="code-panel">
      <div className="code-modes">
        <ModeSwitch agent={agent} mode={mode} onChange={changeMode} />
        <BaseSwitch value={diffBase} onChange={changeBase} />
      </div>
      <div className="code-panel-body">
        {mode === "changes" ? (
          <Changes
            key={agent.id}
            agent={agent}
            selectedPath={selectedPath}
            onSelect={setSelectedPath}
            diffBase={diffBase}
            onDiffBaseChange={changeBase}
            onOpenInEditor={(p) => {
              setSelectedPath(p);
              changeMode("explore");
            }}
          />
        ) : (
          <FilePanel
            agent={agent}
            openPath={selectedPath}
            onOpenPath={setSelectedPath}
            diffBase={diffBase}
          />
        )}
      </div>
    </div>
  );
}

// What the Changes list and every diff measure against: the workspace's
// starting commit (everything the agent has done here) or the latest commit
// (only uncommitted work).
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
        data-tip="Everything changed in this workspace, since it started"
        onClick={() => onChange("fork")}
      >
        <span>All</span>
      </button>
      <button
        role="tab"
        aria-selected={value === "head"}
        className={`cms-seg iflex-center text-xs ${value === "head" ? "active" : ""} tip`}
        data-tip="Only uncommitted work, since the last commit"
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
// ways to look at code here: review what changed, or explore it yourself.
function ModeSwitch({
  agent,
  mode,
  onChange,
}: {
  agent: AgentRecord;
  mode: CodeMode;
  onChange: (m: CodeMode) => void;
}) {
  // Uncommitted files, or commits ahead of the base: work the Changes list shows.
  const hasChanges = useAppStore((s) => {
    const git = s.gitStates[agent.id];
    const dirty = git?.files.length ?? s.gitShortstats[agent.id]?.file_count ?? 0;
    return dirty > 0 || (git?.ahead ?? 0) > 0;
  });
  // Whether the agent is mid-turn — the dot is green & pulsing only then, and
  // goes grey when work stops so it never implies activity that isn't there.
  const busy = useAppStore((s) => isAgentBusy(s, agent.id, agent.status));

  return (
    <div className="code-modeswitch" role="tablist" aria-label="Code view mode">
      <button
        role="tab"
        aria-selected={mode === "changes"}
        className={`cms-seg iflex-center text-xs ${mode === "changes" ? "active" : ""} tip`}
        data-tip="Every file this workspace changed, with its diff"
        onClick={() => onChange("changes")}
      >
        <Icon name="diff" size={12} />
        <span>Changes</span>
        {(hasChanges || busy) && <span className={`cms-live-dot ${busy ? "on" : ""}`}></span>}
      </button>
      <button
        role="tab"
        aria-selected={mode === "explore"}
        className={`cms-seg iflex-center text-xs ${mode === "explore" ? "active" : ""} tip`}
        data-tip="Browse and edit any file in the checkout"
        onClick={() => onChange("explore")}
      >
        <Icon name="folder" size={12} />
        <span>Explore</span>
      </button>
    </div>
  );
}
