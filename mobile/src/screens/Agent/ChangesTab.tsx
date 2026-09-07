import type { AgentRecord } from "@desktop/api/types/agent";
import { Icon } from "../../components/Icon";
import { PrPill } from "../../components/ui";
import { branchOf } from "../../lib/agents";
import { STATUS_LETTER } from "../../lib/diff";
import { useStore } from "../../store";
import { PrCard } from "./PrCard";

export function ChangesTab({ agent }: { agent: AgentRecord }) {
  const git = useStore((s) => s.gitStates[agent.id]);
  const pr = useStore((s) => s.prStates[agent.id]);
  const push = useStore((s) => s.push);
  const openSheet = useStore((s) => s.openSheet);
  const files = git?.files ?? [];

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
            onClick={() => openSheet("pr", { agentId: agent.id })}
          >
            <Icon name="pr" size={17} />
            {pr ? `Commit & push to #${pr.number}` : "Commit & open PR"}
          </button>
        </div>
      )}
    </>
  );
}
