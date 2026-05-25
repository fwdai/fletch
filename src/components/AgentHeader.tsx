import { open } from "@tauri-apps/plugin-dialog";
import type { AgentRecord, AgentView } from "../api";
import { api } from "../api";
import { useAppStore } from "../store";
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

/** Header rendered above each agent pane.
 *
 *  Row 1:   Claude · idle               [+ Add repo] [Custom | Native]
 *  Row 2+:  <id> · <subdir> · <branch> → <parent_branch>     (one per tracked repo)
 *
 *  Single-repo agents show two rows. Multi-repo agents grow by one
 *  row per added repo. Branch info is hidden per-repo until that
 *  repo's first commit lands. */
export function AgentHeader({ agent, view, toggleDisabled }: Props) {
  async function onAddRepo() {
    const picked = await open({
      directory: true,
      multiple: false,
      title: "Select a git repository",
    });
    if (typeof picked !== "string") return;
    try {
      await api.addRepoToAgent(agent.id, picked);
    } catch (e) {
      useAppStore.setState({ lastError: String(e) });
    }
  }

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
          <button
            type="button"
            className="add-repo-btn"
            title="Add another repo to this agent"
            aria-label="Add repo"
            onClick={onAddRepo}
          >
            + Repo
          </button>
          <ViewToggle
            agentId={agent.id}
            current={view}
            disabled={toggleDisabled}
          />
        </div>
      </div>
      {agent.repos.map((repo, idx) => (
        <div
          key={repo.subdir}
          className={`agentheader-row secondary${idx === 0 ? " primary-repo" : ""}`}
        >
          <div className="left">
            {idx === 0 ? (
              <span className="agent-id">{agent.name}</span>
            ) : (
              <span className="agent-id placeholder">+</span>
            )}
            <span className="sep">·</span>
            <span className="subdir">{repo.subdir}</span>
            {repo.branch && (
              <>
                <span className="sep">·</span>
                <span className="branch">{repo.branch}</span>
                {repo.parent_branch && (
                  <>
                    <span className="arrow" aria-hidden="true">
                      →
                    </span>
                    <span className="parent-branch" title="Parent branch">
                      {repo.parent_branch}
                    </span>
                  </>
                )}
              </>
            )}
          </div>
        </div>
      ))}
    </div>
  );
}
