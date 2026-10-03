import type { AgentRecord } from "@desktop/api/types/agent";
import { isCommitAction } from "./GitTab/actions";
import { GitActions, useGitActionList } from "./GitTab/GitActions";

/** The Changes tab's footer: the commit / push / open-PR action for the
 *  working tree and the manual alternative under it, from the same table the
 *  Git tab's footer reads (GitTab/actions.ts) — only narrowed to that family,
 *  since the PR's own remedies belong on the Git tab. Renders nothing when
 *  there is nothing to commit or push. */
export function GitActionFooter({
  agent,
  onDelegated,
}: {
  agent: AgentRecord;
  /** Called once a git action has been handed to the agent, so the screen can
   *  show the chat where the agent's turn plays out. */
  onDelegated?: () => void;
}) {
  const actions = useGitActionList(agent).filter(isCommitAction);
  return <GitActions agent={agent} actions={actions} onDelegated={onDelegated} />;
}
