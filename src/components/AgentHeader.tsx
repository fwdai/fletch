import type { AgentRecord, AgentView } from "../api";
import { ViewToggle } from "./ViewToggle";

interface Props {
  agent: AgentRecord;
  view: AgentView;
  /** Disable the view toggle while a turn is in flight (custom mode);
   *  always enabled in native mode where we can't tell. */
  toggleDisabled?: boolean;
}

// Hardcoded for now — we only support claude. Future providers
// (gemini, codex, …) will add a `provider` field on AgentRecord.
const PROVIDER_LABEL = "Claude";

/** Two-row header shared by both agent views:
 *
 *    Claude · idle                              [Custom | Native]
 *    yosemite · amux/refactor-auth-flow → main
 *
 * Row 1 carries the provider, live status, and the view switcher.
 * Row 2 carries git context — agent id, branch, and the branch this
 * worktree was forked from (the natural merge target). The arrow row
 * is dropped if we don't know the parent branch (detached HEAD at
 * spawn, or pre-`parent_branch` records). */
export function AgentHeader({ agent, view, toggleDisabled }: Props) {
  const parent = agent.parent_branch;
  return (
    <div className="agentheader">
      <div className="agentheader-row primary">
        <div className="left">
          <span className="provider">{PROVIDER_LABEL}</span>
          <span className="sep">·</span>
          <span className="status" data-status={agent.status}>
            {agent.status}
          </span>
        </div>
        <div className="right">
          <ViewToggle
            agentId={agent.id}
            current={view}
            disabled={toggleDisabled}
          />
        </div>
      </div>
      <div className="agentheader-row secondary">
        <div className="left">
          <span className="agent-id">{agent.name}</span>
          {/* Branch (and its arrow to the parent) only appears once
           *  the first user message has triggered branch creation.
           *  Before that we just show the agent id alone. */}
          {agent.branch && (
            <>
              <span className="sep">·</span>
              <span className="branch">{agent.branch}</span>
              {parent && (
                <>
                  <span className="arrow" aria-hidden="true">
                    →
                  </span>
                  <span className="parent-branch" title="Parent branch">
                    {parent}
                  </span>
                </>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}
