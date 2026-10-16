import { useEffect, useState } from "react";
import type { FileStatus } from "@/api";
import { FileDiffView } from "./FileDiffView";
import { Ladder, type LadderBodies } from "./Ladder";
import type { LadderRow } from "./ladderModel";

/** One lane's body: its ladder, or one changed file's diff in place of it.
 *  Mounted per lane (`key={lane.id}`), so everything a user opened — a diff,
 *  a row — is that lane's and resets by construction when another lane comes
 *  into view. */
export function LaneBody({
  agentId,
  rows,
  bodies,
  repoDir,
}: {
  agentId: string;
  rows: LadderRow[];
  bodies: Omit<LadderBodies, "onOpenFile">;
  /** The repo's folder, prefixed to a path for the diff fetch in multi-repo
   *  agents; undefined for single-repo agents. */
  repoDir: string | undefined;
}) {
  // The changed file whose diff is open in place of the ladder; null shows the
  // ladder. Falls back when the file leaves the list (e.g. after a commit).
  const [viewing, setViewing] = useState<string | null>(null);
  const files: FileStatus[] = bodies.files;
  useEffect(() => {
    setViewing((prev) => (prev && files.some((f) => f.path === prev) ? prev : null));
  }, [files]);

  if (viewing) {
    return (
      <FileDiffView
        agentId={agentId}
        files={files}
        path={viewing}
        repoDir={repoDir}
        onSelect={setViewing}
        onBack={() => setViewing(null)}
      />
    );
  }
  return <Ladder rows={rows} bodies={{ ...bodies, onOpenFile: setViewing }} />;
}
