// Walkthrough — guides the reader through the workspace's changes one step at
// a time, inside the Code panel's Changes view. Steps come from the agent's own
// tour (`api.getTour`, written through the `tour_set` mailbox op) when it
// submitted one; otherwise they are derived from the transcript: files in the
// order the agent first edited them, each with the prose the agent wrote just
// before the edit. Nothing here blocks or accepts a change — it only moves the
// diff pane and remembers what has been read.
import "./Walkthrough.css";
import { type ReactNode, useCallback, useState } from "react";
import type { AgentRecord, FileStatus } from "@/api";
import { useBareKeys } from "@/util/hooks";
import { presentAnchor, presentPaths, type StepAnchor, stepIndexFor } from "./model";
import { ResumeStrip } from "./ResumeStrip";
import type { WalkthroughStart } from "./StartButton";
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

/** What the walkthrough last put on screen: a step, over the file it opened
 *  (or over the list, or the file already there, for a step with nothing to
 *  open). While the pane still shows that file the reader is on that step;
 *  once it shows another, they moved on their own. A step needs no file of
 *  its own to be read: an agent's tour may explain before it points. */
interface Shown {
  agentId: string;
  index: number;
  path: string | null;
}

/** The walkthrough as the Changes view places it: `strip` sits above the view
 *  while a step is being read (or, over a diff no step points at, offers the
 *  way back), and `start` is the way in while the file list is showing — the
 *  Changes header draws it on its own row (`StartButton`). */
export function useWalkthroughView({
  agent,
  files,
  currentPath,
  onGo,
  onViewed,
}: WalkthroughProps): { strip: ReactNode; start: WalkthroughStart | null } {
  const { steps, source, index, setIndex } = useWalkthrough(agent, files);
  const [shown, setShown] = useState<Shown | null>(null);

  const started = shown !== null && shown.agentId === agent.id;
  const current =
    shown !== null && shown.agentId === agent.id && shown.path === currentPath
      ? shown.index
      : stepIndexFor(steps, currentPath, index);

  const go = useCallback(
    (i: number) => {
      const step = steps[i];
      if (!step) return;
      setIndex(i);
      const anchor = presentAnchor(step, files);
      setShown({ agentId: agent.id, index: i, path: anchor ? anchor.path : currentPath });
      if (anchor) onGo(anchor.path, anchor.range);
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
      setShown({ agentId: agent.id, index: current, path: anchor.path });
      onGo(anchor.path, anchor.range);
    },
    [current, setIndex, onGo, agent.id],
  );

  // `j` / `k` step while a step is on screen or a diff is up; a chord with a
  // modifier is someone else's.
  const onStrip = current !== -1;
  useBareKeys(steps.length > 0 && (onStrip || currentPath !== null) ? { j: next, k: prev } : null);

  if (steps.length === 0) return { strip: null, start: null };

  if (onStrip) {
    return {
      strip: (
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
      ),
      start: null,
    };
  }

  if (currentPath === null) {
    return {
      strip: null,
      start: { count: steps.length, source, started, onStart: () => go(index) },
    };
  }

  return {
    strip: <ResumeStrip step={index + 1} started={started} onResume={() => go(index)} />,
    start: null,
  };
}
