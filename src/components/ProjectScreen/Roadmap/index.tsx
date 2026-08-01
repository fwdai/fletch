import { Board } from "./Board";
import { Thread } from "./Thread";
import type { RoadmapState } from "./useRoadmap";

export type { RoadmapState } from "./useRoadmap";
export { useRoadmap } from "./useRoadmap";

/** The Roadmap tab of the project page: a conversation with the project's PM
 *  agent on the left, the board it maintains on the right.
 *
 *  State is owned by `useRoadmap` in the parent, because the page header shows
 *  the same counts the board does. */
export function Roadmap({ roadmap }: { roadmap: RoadmapState }) {
  return (
    <div className="rm">
      <Thread roadmap={roadmap} />
      <Board roadmap={roadmap} />
    </div>
  );
}
