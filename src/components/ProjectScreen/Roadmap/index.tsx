import { useRef } from "react";
import { ROADMAP_MIN_BOARD, ROADMAP_MIN_THREAD } from "@/storage/preferences";
import { useAppStore } from "@/store";
import { useSplitter } from "@/util/splitter";
import { Board } from "./Board";
import { Thread } from "./Thread";
import type { RoadmapState } from "./useRoadmap";

export type { RoadmapState } from "./useRoadmap";
export { useRoadmap } from "./useRoadmap";

/** The Roadmap tab of the project page: a real chat with the project's PM
 *  agent on the left, the board it maintains on the right.
 *
 *  The columns split the page evenly and are resizable from the rule between
 *  them. Which of the two you want wide swings through the day — shaping a
 *  proposal is a reading task, triaging the board is a scanning one — and no
 *  single ratio serves both. The drag reuses the app shell's splitter, so it
 *  behaves exactly like the sidebar's; the width is persisted on drag end.
 *
 *  Board state is owned by `useRoadmap` in the parent, because the page header
 *  shows the same counts the board does. The chats are owned by the Thread —
 *  nothing above it needs them. */
export function Roadmap({ roadmap, repoPath }: { roadmap: RoadmapState; repoPath: string }) {
  const width = useAppStore((s) => s.roadmapBoardWidth);
  const setWidth = useAppStore((s) => s.setRoadmapBoardWidth);
  const commitWidth = useAppStore((s) => s.commitRoadmapBoardWidth);
  const board = useRef<HTMLElement>(null);

  const onDrag = useSplitter(
    // Measured off the element rather than read from `width`: until the first
    // drag the board is sized by CSS (50%) and the store holds null, so there
    // is no number to start the drag from.
    () => board.current?.getBoundingClientRect().width ?? 0,
    setWidth,
    "right",
    commitWidth,
    {
      min: ROADMAP_MIN_BOARD,
      // The board may take everything the chat doesn't need. Measured from the
      // splitter's parent — this row — whose width is fixed for the drag.
      max: (el) => (el.parentElement?.getBoundingClientRect().width ?? 0) - ROADMAP_MIN_THREAD,
    },
  );

  return (
    <div className="rm">
      <Thread roadmap={roadmap} repoPath={repoPath} />
      <div className="splitter" onMouseDown={onDrag} />
      <Board roadmap={roadmap} repoPath={repoPath} width={width} asideRef={board} />
    </div>
  );
}
