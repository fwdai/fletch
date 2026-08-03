import { MergeTime } from "./MergeTime";
import { ProjectPulse } from "./ProjectPulse";
import { RecentlyShipped } from "./RecentlyShipped";
import { Spend } from "./Spend";

/** The Activity tab: what this project has actually done, as against the
 *  Roadmap tab's what it is going to do.
 *
 *  Four sections, ordered widest lens to narrowest — a year of daily activity,
 *  then how fast work lands, then what it costs, then the specific items that
 *  shipped. Each owns its own query and loading state rather than sharing one
 *  gate: the pulse's token fold walks every session's transcript and takes far
 *  longer than the three indexed queries beside it, so one shared gate would
 *  hold the whole page at the speed of its slowest part. */
export function Activity({ projectId }: { projectId: string }) {
  return (
    <>
      <ProjectPulse projectId={projectId} />
      <MergeTime projectId={projectId} />
      <Spend projectId={projectId} />
      <RecentlyShipped projectId={projectId} />
    </>
  );
}
