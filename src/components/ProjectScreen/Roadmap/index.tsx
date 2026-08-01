import { Board } from "./Board";
import { Thread } from "./Thread";
import type { RoadmapState } from "./useRoadmap";

export type { RoadmapState } from "./useRoadmap";
export { useRoadmap } from "./useRoadmap";

/** The Roadmap tab of the project page: a real chat with the project's PM
 *  agent on the left, the board it maintains on the right.
 *
 *  Board state is owned by `useRoadmap` in the parent, because the page header
 *  shows the same counts the board does. The chats are owned by the Thread —
 *  nothing above it needs them. */
export function Roadmap({ roadmap, repoPath }: { roadmap: RoadmapState; repoPath: string }) {
  return (
    <div className="rm">
      <Thread roadmap={roadmap} repoPath={repoPath} />
      <Board roadmap={roadmap} repoPath={repoPath} />
    </div>
  );
}
