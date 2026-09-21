import { invoke, invokeLocal } from "../invoke";
import type {
  IssueComment,
  IssueSource,
  LinearStatus,
  LinearTeam,
  TrackerIssue,
} from "../types/issues";

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
  /** The picked issue's discussion for the composed brief — the newest ~20
   *  comments, oldest-first, bodies clamped. Degrades to `[]` (never errors),
   *  so a failed fetch only loses the discussion section. */
  issueComments: (repoPath: string, source: IssueSource, key: string) =>
    invoke<IssueComment[]>("issue_comments", { repoPath, source, key }),
  /** Re-tag a running agent with the issue it's working (a mid-session pick
   *  in the composer), so its eventual PR carries the closing trailer. */
  setAgentIssueRef: (agentId: string, issueRef: string) =>
    invoke<void>("set_agent_issue_ref", { agentId, issueRef }),
  // Linear is an account on THIS Mac: the personal API key lives in this
  // machine's keychain and none of the `linear_*` ops is on a host's table
  // (docs/remote-protocol.md), so routing them would answer `UNKNOWN_OP` the
  // moment a host is the active environment.
  linearStatus: () => invokeLocal<LinearStatus>("linear_status"),
  /** Validate + store a Linear personal API key. Rejects on a bad key. */
  linearConnect: (apiKey: string) => invokeLocal<LinearStatus>("linear_connect", { apiKey }),
  linearDisconnect: () => invokeLocal<void>("linear_disconnect"),
  linearListTeams: () => invokeLocal<LinearTeam[]>("linear_list_teams"),
};
