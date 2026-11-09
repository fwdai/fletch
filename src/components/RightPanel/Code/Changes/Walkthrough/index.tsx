// Walkthrough — guides the reader through the workspace's changes one step at
// a time, inside the Code panel's Changes view. Steps come from the agent's own
// tour (`api.getTour`, written through the `tour_set` mailbox op) when it
// submitted one; otherwise they are derived from the transcript: files in the
// order the agent first edited them, each with the prose the agent wrote just
// before the edit. Nothing here blocks or accepts a change — it only moves the
// diff pane and remembers what has been read.
import "./Walkthrough.css";
import { useCallback, useState } from "react";
import type { AgentRecord, FileStatus } from "@/api";
import { useBareKeys } from "@/util/hooks";
import { presentAnchor, presentPaths, type StepAnchor, stepIndexFor } from "./model";
import { ResumeStrip } from "./ResumeStrip";
import { StartCard } from "./StartCard";
import { StepStrip } from "./StepStrip";
import { useWalkthrough } from "./useWalkthrough";

/** A 1-indexed, inclusive line range on a file's new side. */
export interface WalkthroughRange {
  start: number;
  end: number;
}

export interface WalkthroughProps {
  agent: AgentRecord;
  /** The changed files in the Changes view's current scope, in display order. */
  files: FileStatus[];
  /** The file the diff pane shows now; null while the file list is showing. */
  currentPath: string | null;
  /** Show `path` in the diff pane, scrolled to `range` when one is given. */
  onGo: (path: string, range?: WalkthroughRange) => void;
  /** Mark `paths` as viewed — moving past a step counts as having read it. */
  onViewed?: (paths: string[]) => void;
}

/** A step with none of its files in view, reached with Prev / Next / Resume.
 *  The diff pane can't go there, so the strip shows the step over the file the
 *  reader was on, until they open another. */
interface Parked {
  agentId: string;
  index: number;
  path: string | null;
}

export function Walkthrough({ agent, files, currentPath, onGo, onViewed }: WalkthroughProps) {
  const { steps, source, index, setIndex } = useWalkthrough(agent, files);
  const [parked, setParked] = useState<Parked | null>(null);

  const isParked =
    parked?.agentId === agent.id && parked.index === index && parked.path === currentPath;
  const current = isParked ? index : stepIndexFor(steps, currentPath, index);

  const go = useCallback(
    (i: number) => {
      const step = steps[i];
      if (!step) return;
      setIndex(i);
      const anchor = presentAnchor(step, files);
      if (anchor) {
        setParked(null);
        onGo(anchor.path, anchor.range);
      } else {
        setParked({ agentId: agent.id, index: i, path: currentPath });
      }
    },
    [steps, files, setIndex, onGo, agent.id, currentPath],
  );

  // Off the walkthrough, "next" is back onto it.
  const next = useCallback(() => {
    if (current === -1) return go(index);
    if (current >= steps.length - 1) return;
    const read = presentPaths(steps[current], files);
    if (read.length > 0) onViewed?.(read);
    go(current + 1);
  }, [current, index, steps, files, onViewed, go]);

  const prev = useCallback(() => {
    if (current > 0) go(current - 1);
  }, [current, go]);

  // A chip opens a file of the step on screen, which may sit in other steps
  // too: pin the reader to this one so the strip doesn't jump to another.
  const open = useCallback(
    (anchor: StepAnchor) => {
      setIndex(current);
      setParked(null);
      onGo(anchor.path, anchor.range);
    },
    [current, setIndex, onGo],
  );

  // `j` / `k` step while the diff pane is up; a chord with a modifier is
  // someone else's.
  useBareKeys(currentPath !== null && steps.length > 0 ? { j: next, k: prev } : null);

  if (steps.length === 0) return null;

  if (currentPath === null) {
    const first = steps.findIndex((s) => presentAnchor(s, files) !== undefined);
    return (
      <StartCard
        count={steps.length}
        source={source}
        onStart={first === -1 ? undefined : () => go(first)}
      />
    );
  }

  if (current === -1) return <ResumeStrip step={index + 1} onResume={() => go(index)} />;

  return (
    <StepStrip
      step={steps[current]}
      index={current}
      total={steps.length}
      files={files}
      currentPath={currentPath}
      onOpen={open}
      onPrev={prev}
      onNext={next}
    />
  );
}
