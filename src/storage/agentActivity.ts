import { type ActivityStamps, loadStamps, mergeStamps } from "./activityStamps";

const AGENT_ACTIVITY_KEY = "q2:agentActivity";

/** When a turn last started in each workspace, keyed by agent id → ms epoch.
 *  The sidebar's "older" fold reads it alongside the launch time, so a
 *  workspace resumed today is recent however long ago it was created. */
export type AgentActivity = ActivityStamps;

export function loadAgentActivity(): AgentActivity {
  return loadStamps(AGENT_ACTIVITY_KEY);
}

/** Merge `stamps` into the saved map and persist. Agents are archived and
 *  deleted all the time, so stamps older than `keepSince` — past the point
 *  where they could still make a workspace recent — are dropped rather than
 *  kept forever. Returns the merged map. */
export function stampAgentActivity(stamps: AgentActivity, keepSince: number): AgentActivity {
  return mergeStamps(AGENT_ACTIVITY_KEY, stamps, keepSince);
}
