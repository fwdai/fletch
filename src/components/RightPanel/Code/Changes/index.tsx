// Changes — the Code panel's first mode: every file this workspace changed, in
// the scope the All / Uncommitted switch picks, as a list; a row opens its
// diff. Follow keeps the diff on the file the agent is editing during a turn,
// and the Walkthrough rides on top of both views, moving the diff pane itself.
import { useEffect, useRef, useState } from "react";
import type { AgentRecord, DiffBaseMode } from "@/api";
import { isAgentBusy } from "@/helpers";
import { useAppStore } from "@/store";
import { ChangedFileDiff } from "./ChangedFileDiff";
import { ChangedFileList } from "./ChangedFileList";
import { ChangesHeader, DegradedHint } from "./ChangesHeader";
import { EmptyChanges } from "./EmptyChanges";
import { useChangedFiles } from "./useChangedFiles";
import { useLiveFile } from "./useLiveFile";
import { useViewed } from "./viewed";
import { Walkthrough, type WalkthroughRange } from "./Walkthrough";

interface ChangesProps {
  agent: AgentRecord;
  /** The file shared with Explore, owned by the Code panel so it survives a
   *  mode switch. May be null or name a file that isn't in the list. */
  selectedPath: string | null;
  /** Must be stable: the follow effect depends on it. */
  onSelect: (path: string) => void;
  /** The scope: what the list and every diff measure against. */
  diffBase: DiffBaseMode;
  onDiffBaseChange: (base: DiffBaseMode) => void;
  onOpenInEditor: (path: string) => void;
}

/** Mount one per agent (`key={agent.id}`): the view, follow and the polled
 *  list are that agent's and reset by construction on a switch. */
export function Changes({
  agent,
  selectedPath,
  onSelect,
  diffBase,
  onDiffBaseChange,
  onOpenInEditor,
}: ChangesProps) {
  // Same signal the chat's "thinking" spinner uses, so live state appears and
  // clears in lockstep with the rest of the UI.
  const busy = useAppStore((s) => isAgentBusy(s, agent.id, agent.status));
  const { files, additions, deletions, degraded, loaded } = useChangedFiles(
    agent.id,
    diffBase,
    busy,
  );
  const liveFile = useLiveFile(`${agent.id}:${diffBase}`, files, busy);
  const viewed = useViewed(agent.id, diffBase, files);

  const [view, setView] = useState<"list" | "diff">("list");
  const [follow, setFollow] = useState(true);
  const [focus, setFocus] = useState<{ path: string; range: WalkthroughRange } | null>(null);
  // Entered mid-turn with Follow armed: open the live file's diff as soon as
  // it is known, instead of the list. Any navigation first cancels it.
  const enterLive = useRef(busy);

  useEffect(() => {
    if (!busy) {
      enterLive.current = false;
      return;
    }
    if (!follow || !liveFile) return;
    if (liveFile !== selectedPath) onSelect(liveFile);
    if (enterLive.current) {
      enterLive.current = false;
      setView("diff");
    }
  }, [busy, follow, liveFile, selectedPath, onSelect]);

  // During a turn, picking the live file re-arms Follow and picking any other
  // pauses it (the user is reading something specific). While idle there is
  // nothing to follow, so a pick only browses and an armed Follow stays armed
  // for the next turn.
  const open = (path: string, range?: WalkthroughRange) => {
    enterLive.current = false;
    onSelect(path);
    setView("diff");
    // Copied so every request is a new one to the diff pane, even a repeat.
    setFocus(range ? { path, range: { ...range } } : null);
    if (busy) setFollow(path === liveFile);
  };

  const back = () => {
    enterLive.current = false;
    setView("list");
    setFocus(null);
  };

  const toggleFollow = () => {
    if (!follow && busy && liveFile) open(liveFile);
    else setFollow(!follow);
  };

  // A file that leaves the list (committed under Uncommitted, reverted) drops
  // back to the list; while a new scope's list loads, the diff stays put.
  const current = files.find((f) => f.path === selectedPath);
  const showDiff = view === "diff" && selectedPath != null && (current != null || !loaded);
  // In the fork scope a failed fetch means the list is a subset; under
  // Uncommitted the fallback is the exact list, so there is nothing to say.
  const partial = degraded && diffBase === "fork";

  const walkthrough = (
    <Walkthrough
      agent={agent}
      files={files}
      currentPath={showDiff ? selectedPath : null}
      onGo={open}
      onViewed={viewed.markViewed}
    />
  );

  if (showDiff) {
    return (
      <div className="code-wrap">
        {walkthrough}
        <ChangedFileDiff
          agentId={agent.id}
          files={files}
          path={selectedPath}
          base={diffBase}
          onSelect={(p) => open(p)}
          onBack={back}
          onEdit={onOpenInEditor}
          viewed={current && { on: viewed.isViewed(current), toggle: () => viewed.toggle(current) }}
          live={follow && busy && selectedPath === liveFile}
          focusRange={focus?.path === selectedPath ? focus.range : undefined}
        />
      </div>
    );
  }

  return (
    <div className="code-wrap">
      {walkthrough}
      {!loaded ? (
        <div className="empty-msg" style={{ margin: "auto" }}>
          <div className="et">Loading…</div>
        </div>
      ) : files.length === 0 ? (
        <>
          {partial && <DegradedHint />}
          <EmptyChanges base={diffBase} onShowAll={() => onDiffBaseChange("fork")} />
        </>
      ) : (
        <>
          <ChangesHeader
            count={files.length}
            additions={additions}
            deletions={deletions}
            follow={follow}
            busy={busy}
            onToggleFollow={toggleFollow}
            degraded={partial}
          />
          <div className="code-list">
            <ChangedFileList
              files={files}
              onOpen={(p) => open(p)}
              activePath={selectedPath}
              liveFile={liveFile}
              isViewed={viewed.isViewed}
              onToggleViewed={viewed.toggle}
            />
          </div>
        </>
      )}
    </div>
  );
}
