import type { AgentRecord } from "@desktop/api/types/agent";
import { Icon } from "@desktop/components/Icon";
import { PrPill } from "../../components/ui";
import { refLabel } from "../../lib/agents";
import { STATUS_LETTER } from "../../lib/diff";
import { useStore } from "../../store";
import { GitActionFooter } from "./GitActionFooter";

/** The working tree's diff: one row per changed file, each opening its diff,
 *  and a way into the whole checkout. Branch and PR state live on the Git tab. */
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
  const files = git?.files ?? [];

  return (
    <>
      <div className="scroll">
        {files.length > 0 && (
          <div className="ch-head">
            <div className="ch-sum">
              <span className="big">
                <span>{files.length} files</span>
                <span className="add">+{git?.additions ?? 0}</span>
                <span className="rem">−{git?.deletions ?? 0}</span>
              </span>
            </div>
          </div>
        )}
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
                `${git.unpushed} commit(s) on ${refLabel(git)}, nothing pending.`
              ) : (
                "No changes yet."
              )}
            </div>
          )}
          <div className="card">
            <button
              type="button"
              className="row"
              onClick={() => push("code", { agentId: agent.id })}
            >
              <span className="pm lg" style={{ background: "var(--bg-2)", color: "var(--fg-2)" }}>
                <Icon name="folder" size={15} />
              </span>
              <div className="main">
                <div className="lbl">Browse all files</div>
                <div className="sub">The whole checkout, read-only</div>
              </div>
              <Icon name="chevR" size={16} className="chev" />
            </button>
          </div>
        </div>
      </div>
      <GitActionFooter agent={agent} onDelegated={onDelegated} />
    </>
  );
}
