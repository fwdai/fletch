import type { AgentRecord } from "@desktop/api/types/agent";
import { Icon } from "../../components/Icon";
import { PrPill } from "../../components/ui";
import { baseOf, branchOf, isBusy } from "../../lib/agents";
import { STATUS_LETTER } from "../../lib/diff";
import { useStore } from "../../store";
import { PrCard } from "./PrCard";

export function ChangesTab({
  agent,
  onDelegated,
}: {
  agent: AgentRecord;
  /** Called once a git action has been handed to the agent, so the screen can
   *  show the chat where the agent's turn plays out. */
  onDelegated?: () => void;
}) {
  const git = useStore((s) => s.gitStates[agent.id]);
  const pr = useStore((s) => s.prStates[agent.id]);
  const push = useStore((s) => s.push);
  const openSheet = useStore((s) => s.openSheet);
  const delegateGit = useStore((s) => s.delegateGit);
  const sending = useStore((s) => !!s.busy[agent.id]);
  const files = git?.files ?? [];
  // A trigger sent mid-turn folds into the running turn instead of running as
  // its own (the desktop queues it until idle); v1 on the phone simply waits.
  const busy = sending || isBusy(agent);

  // Mirrors the desktop's default: the agent commits and opens a PR; with a PR
  // already open, "open PR" degrades to push (that's what updates it); with
  // everything committed but no PR, the agent names the branch and opens one.
  const action = pr
    ? { name: "commit-push", label: `Commit & push to #${pr.number}` }
    : files.length
      ? { name: "commit-pr", label: "Commit & open PR" }
      : { name: "open-pr", label: "Open PR" };

  const delegate = async () => {
    await delegateGit(agent.id, action.name, { base: baseOf(agent) });
    onDelegated?.();
  };

  return (
    <>
      <div className="scroll">
        <div className="ch-head">
          {pr && <PrCard agentId={agent.id} pr={pr} />}
          <div className="ch-sum">
            <span className={`pill mono ${files.length ? "warn" : "ok"}`}>
              {files.length ? "uncommitted" : "clean"}
            </span>
            <span className="pill mono">
              <Icon name="branch" size={11} />
              {branchOf(agent)}
            </span>
            {git && git.unpushed > 0 && (
              <span className="pill mono">
                <Icon name="commit" size={11} />
                {git.unpushed} unpushed
              </span>
            )}
            {files.length > 0 && (
              <span className="big">
                <span>{files.length} files</span>
                <span className="add">+{git?.additions ?? 0}</span>
                <span className="rem">−{git?.deletions ?? 0}</span>
              </span>
            )}
          </div>
        </div>
        <div className="ch-list">
          {files.length > 0 ? (
            <div className="card">
              {files.map((f) => {
                const parts = f.path.split("/");
                const name = parts.pop();
                return (
                  <button
                    type="button"
                    key={f.path}
                    className="cf"
                    onClick={() => push("diff", { agentId: agent.id, path: f.path })}
                  >
                    <span className={`k ${STATUS_LETTER[f.kind] ?? "M"}`}>
                      {STATUS_LETTER[f.kind] ?? "M"}
                    </span>
                    <span className="p">
                      <span>{parts.length ? `${parts.join("/")}/` : ""}</span>
                      <b>{name}</b>
                    </span>
                    <span className="d">
                      <span className="add">+{f.additions}</span>
                      {f.deletions > 0 && <span className="rem">−{f.deletions}</span>}
                    </span>
                    <Icon name="chevR" size={14} className="chev" />
                  </button>
                );
              })}
            </div>
          ) : (
            <div className="empty">
              <b>Working tree is clean</b>
              {pr ? (
                <>
                  Everything is on PR <PrPill pr={pr} />.
                </>
              ) : git?.unpushed ? (
                `${git.unpushed} commit(s) on ${branchOf(agent)}, nothing pending.`
              ) : (
                "No changes yet."
              )}
            </div>
          )}
        </div>
      </div>
      {(files.length > 0 || (git?.unpushed ?? 0) > 0) && (
        <div className="ch-foot">
          <button
            type="button"
            className="btn primary"
            onClick={() => void delegate()}
            disabled={busy}
          >
            <Icon name="pr" size={17} />
            {busy ? "Agent is busy…" : `${action.label} with agent`}
          </button>
          <button
            type="button"
            className="alt"
            onClick={() => openSheet("pr", { agentId: agent.id })}
          >
            Write the message yourself
          </button>
        </div>
      )}
    </>
  );
}
