import type { AgentRecord, Workspace } from "@/api";

/**
 * Fold a just-spawned agent's record into the workspace snapshot, replacing any
 * record that already holds its id.
 *
 * Agent ids are landmark names, and the backend recycles the names of archived
 * agents: when a spawn reuses one, it evicts the archived row and inserts the
 * new one under the same primary key. Until the next `getWorkspace()` lands,
 * this store still holds the *archived* record under that id — so selecting the
 * new agent would mount its chat against the old record (wrong provider, model,
 * effort, custom agent), and because the id (and so the React key) never
 * changes, the composer's mount-time state would then never catch up. Every
 * spawn site applies the returned record here in the same update that selects
 * it, so the chat only ever mounts against the real record. The refresh that
 * follows is still authoritative for everything else in the snapshot.
 */
export function adoptSpawnedAgent(workspace: Workspace | null, rec: AgentRecord): Workspace | null {
  if (!workspace) return workspace;
  const others = workspace.agents.filter((a) => a.id !== rec.id);
  return { ...workspace, agents: [rec, ...others] };
}
