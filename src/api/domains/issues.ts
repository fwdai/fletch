import { invoke } from "../invoke";
import type { LinearStatus, LinearTeam, TrackerIssue } from "../types/issues";

export const issuesApi = {
  /** Open, relevant issues for a repo across every configured tracker source
   *  (GitHub by origin, Linear by the project's configured team): not closed,
   *  and unassigned or assigned to the signed-in user — never someone else's
   *  work. Sources degrade quietly to nothing — `[]` covers "nothing
   *  connected" and "no open issues" alike, so callers never branch on a
   *  connection error. */
  listTrackerIssues: (repoPath: string, linearTeamId?: string) =>
    invoke<TrackerIssue[]>("list_tracker_issues", {
      repoPath,
      linearTeamId: linearTeamId ?? null,
    }),
  /** Re-tag a running agent with the issue it's working (a mid-session pick
   *  in the composer), so its eventual PR carries the closing trailer. */
  setAgentIssueRef: (agentId: string, issueRef: string) =>
    invoke<void>("set_agent_issue_ref", { agentId, issueRef }),
  linearStatus: () => invoke<LinearStatus>("linear_status"),
  /** Validate + store a Linear personal API key. Rejects on a bad key. */
  linearConnect: (apiKey: string) => invoke<LinearStatus>("linear_connect", { apiKey }),
  linearDisconnect: () => invoke<void>("linear_disconnect"),
  linearListTeams: () => invoke<LinearTeam[]>("linear_list_teams"),
};
