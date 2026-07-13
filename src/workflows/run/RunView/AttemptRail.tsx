// RunView/AttemptRail.tsx — the left rail of steps and their attempts (spec
// §14.2). Every attempt is listed and clickable (it opens that attempt's
// preserved chat); abandoned attempts are dimmed, never hidden — a retry or a
// loop iteration adds an attempt, it never rewrites history.

import type { WfStepExec } from "../../../api";
import { Icon } from "../../../components/Icon";
import { AgentAvatar } from "../../builder/AgentAvatar";
import type { ResolvedAgent } from "../../shared";
import { attemptChip } from "../status";
import type { StepDesc } from "./flatten";

export function AttemptRail({
  steps,
  attempts,
  resolve,
  selectedId,
  onSelect,
}: {
  steps: StepDesc[];
  attempts: WfStepExec[];
  resolve: (alias: string) => ResolvedAgent | null;
  selectedId: string | null;
  onSelect: (attempt: WfStepExec) => void;
}) {
  return (
    <div className="wf-rail">
      {steps.map((step, i) => {
        const rows = attempts
          .filter((a) => a.step_id === step.id)
          .sort((a, b) => a.iteration - b.iteration || a.attempt - b.attempt);
        const latest = rows[rows.length - 1];
        const head = attemptChip(latest?.status ?? "pending");
        const agent = resolve(step.agentAlias);

        return (
          <div className="wf-step-group" key={step.id}>
            <div className="wf-step-head" title={step.goal}>
              <span className="wf-step-idx">{String(i + 1).padStart(2, "0")}</span>
              {agent ? (
                <AgentAvatar
                  custom={agent.custom}
                  slug={agent.providerId}
                  short={agent.short}
                  hue={agent.hue}
                  size={20}
                />
              ) : (
                <span className="wf-step-q">?</span>
              )}
              <span className="wf-step-name">{agent?.name ?? step.agentAlias ?? "Unassigned"}</span>
              {step.container && <span className="wf-step-tag">{step.container}</span>}
              <span className="wf-step-chip" style={{ color: head.tone }} title={head.label}>
                <Icon name={head.icon} size={11} />
              </span>
            </div>

            {rows.length > 0 && (
              <div className="wf-attempts">
                {rows.map((row) => {
                  const chip = attemptChip(row.status);
                  const dimmed = row.status === "abandoned";
                  return (
                    <button
                      type="button"
                      key={row.id}
                      className={`wf-attempt ${selectedId === row.id ? "sel" : ""} ${dimmed ? "dim" : ""}`}
                      onClick={() => onSelect(row)}
                    >
                      <span className="wf-attempt-label">
                        attempt {row.attempt}
                        {rows.some((r) => r.iteration > 0) && ` · iter ${row.iteration + 1}`}
                      </span>
                      <span className="wf-attempt-chip" style={{ color: chip.tone }}>
                        <Icon name={chip.icon} size={10} />
                        {chip.label}
                      </span>
                    </button>
                  );
                })}
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
