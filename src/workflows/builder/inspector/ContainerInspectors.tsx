// ContainerInspectors.tsx — inspector panes for the three container blocks.
// Branch/child steps are edited by selecting them on the canvas; these panes
// hold the container's own knobs (join/integrate/limits, loop exit, the
// orchestrator's brief and dynamic-children/compose settings).

import { STEP_COMMS } from "../../data";
import type { CommsCap } from "../../spec";
import { ContainerErrors } from "../blocks/ContainerErrors";
import type { BuilderCtx } from "../ctx";
import { loopExitCandidates } from "../edits";
import type { ELoop, EOrchestrate, EParallel } from "../model";
import { AgentButton, Field } from "./bits";

export function ParallelInspector({ block, ctx }: { block: EParallel; ctx: BuilderCtx }) {
  return (
    <>
      <ContainerErrors errors={ctx.errorsFor(block.nid)} />

      <Field label="Join" hint="when does the stage complete">
        <select
          className="ca-select"
          value={block.join}
          onChange={(e) => ctx.patchBlock(block.nid, { join: e.target.value as EParallel["join"] })}
        >
          <option value="all">all — every branch must finish</option>
          <option value="any">any — the first finished branch wins</option>
        </select>
      </Field>

      <Field label="Integrate">
        <select
          className="ca-select"
          value={block.integrate}
          onChange={(e) =>
            ctx.patchBlock(block.nid, { integrate: e.target.value as EParallel["integrate"] })
          }
        >
          <option value="none">none — branches stay on their own worktrees</option>
          <option value="merge">merge — combine branch results</option>
        </select>
      </Field>

      <Field label="Max at once" hint="blank runs all branches">
        <input
          className="ca-input"
          type="number"
          min={1}
          placeholder="all"
          value={block.maxConcurrent ?? ""}
          onChange={(e) =>
            ctx.patchBlock(block.nid, {
              maxConcurrent: e.target.value.trim() === "" ? null : Number(e.target.value),
            })
          }
        />
      </Field>

      <div className="wb-field-note">
        Select a branch on the canvas to edit its agent, instructions, and gate. Add branches with
        the + button inside the block.
      </div>
    </>
  );
}

export function LoopInspector({ block, ctx }: { block: ELoop; ctx: BuilderCtx }) {
  const candidates = loopExitCandidates(block.body);
  return (
    <>
      <ContainerErrors errors={ctx.errorsFor(block.nid)} />

      <Field label="Max iterations">
        <select
          className="ca-select"
          value={block.max}
          onChange={(e) => ctx.patchBlock(block.nid, { max: Number(e.target.value) })}
        >
          {[1, 2, 3, 4, 5, 6, 8, 10].map((n) => (
            <option key={n} value={n}>
              {n}×
            </option>
          ))}
        </select>
      </Field>

      <Field label="Exit when" hint="the step whose verdict ends the loop">
        <select
          className="ca-select"
          value={block.untilNid ?? ""}
          onChange={(e) => ctx.patchBlock(block.nid, { untilNid: e.target.value || null })}
        >
          <option value="">choose step…</option>
          {candidates.map((c) => (
            <option key={c.nid} value={c.nid}>
              {c.label} is done
            </option>
          ))}
        </select>
      </Field>

      <div className="wb-field-note">
        The body repeats until the chosen step writes a <span className="mono">done</span> verdict,
        up to the max. The exit step must use the <b>verdict</b> gate.
      </div>
    </>
  );
}

export function OrchestrateInspector({ block, ctx }: { block: EOrchestrate; ctx: BuilderCtx }) {
  const lead = ctx.resolve(block.agent);
  const child = ctx.resolve(block.children?.agent ?? null);

  const toggleComms = (cap: CommsCap) => {
    const has = block.comms.includes(cap);
    ctx.patchBlock(block.nid, {
      comms: has ? block.comms.filter((c) => c !== cap) : [...block.comms, cap],
    });
  };

  return (
    <>
      <ContainerErrors errors={ctx.errorsFor(block.nid)} />

      <Field label="Orchestrator" required>
        <AgentButton
          agent={lead}
          placeholder="Choose an orchestrator"
          onClick={(e) => ctx.openAgent(block.nid, "orchestrator", e)}
        />
      </Field>

      <Field label="Coordination brief" hint="how to run the children">
        <textarea
          className="wb-insp-textarea"
          value={block.goal}
          placeholder="How should the orchestrator coordinate its children?"
          onChange={(e) => ctx.patchBlock(block.nid, { goal: e.target.value })}
        />
      </Field>

      <Field label="Join">
        <select
          className="ca-select"
          value={block.join}
          onChange={(e) =>
            ctx.patchBlock(block.nid, { join: e.target.value as EOrchestrate["join"] })
          }
        >
          <option value="all">all — every child must finish</option>
          <option value="any">any — the first finished child wins</option>
        </select>
      </Field>

      {/* The engine runs orchestrate with integrate: none only (§6.6); a legacy
          definition may still carry merge — surface it so it can be switched off. */}
      {block.integrate === "merge" && (
        <Field label="Integrate">
          <select
            className="ca-select"
            value={block.integrate}
            onChange={(e) =>
              ctx.patchBlock(block.nid, { integrate: e.target.value as EOrchestrate["integrate"] })
            }
          >
            <option value="none">none</option>
            <option value="merge" disabled>
              merge (unsupported)
            </option>
          </select>
        </Field>
      )}

      <Field label="Children may">
        <div className="wb-comms">
          {STEP_COMMS.map((c) => (
            <button
              key={c.id}
              className={`wb-comm ${block.comms.includes(c.id) ? "on" : ""} tip`}
              data-tip-down
              data-tip={c.note}
              onClick={() => toggleComms(c.id)}
            >
              {c.label}
            </button>
          ))}
        </div>
      </Field>

      <Field label="Dynamic children" hint="spawned by the orchestrator">
        <div className="wb-box">
          <label className="wb-toggle">
            <input
              type="checkbox"
              checked={!!block.children}
              onChange={(e) =>
                ctx.patchBlock(block.nid, {
                  children: e.target.checked ? { agent: null, max: 3 } : null,
                })
              }
            />
            Let the orchestrator spawn children
          </label>
          {block.children && (
            <div className="wb-box-cfg">
              <AgentButton
                agent={child}
                placeholder="Choose the child agent"
                onClick={(e) => ctx.openAgent(block.nid, "child", e)}
              />
              <label className="wb-budget-field">
                <span>Max children</span>
                <input
                  className="ca-input sm"
                  type="number"
                  min={1}
                  value={block.children.max}
                  onChange={(e) =>
                    ctx.patchBlock(block.nid, {
                      children: {
                        agent: block.children?.agent ?? null,
                        max: Number(e.target.value),
                      },
                    })
                  }
                />
              </label>
            </div>
          )}
        </div>
      </Field>

      <Field label="Sub-workflows" hint="compose whole workflows">
        <div className="wb-box">
          <label className="wb-toggle">
            <input
              type="checkbox"
              checked={!!block.compose}
              onChange={(e) =>
                ctx.patchBlock(block.nid, {
                  compose: e.target.checked ? { maxSubRuns: 2, maxDepth: 2 } : null,
                })
              }
            />
            Allow launching sub-workflows
          </label>
          {block.compose && (
            <div className="wb-box-cfg row">
              <label className="wb-budget-field">
                <span>Sub-runs</span>
                <input
                  className="ca-input sm"
                  type="number"
                  min={1}
                  value={block.compose.maxSubRuns}
                  onChange={(e) =>
                    ctx.patchBlock(block.nid, {
                      compose: {
                        maxSubRuns: Number(e.target.value),
                        maxDepth: block.compose?.maxDepth ?? 2,
                      },
                    })
                  }
                />
              </label>
              <label className="wb-budget-field">
                <span>Depth</span>
                <select
                  className="ca-select sm"
                  value={block.compose.maxDepth}
                  onChange={(e) =>
                    ctx.patchBlock(block.nid, {
                      compose: {
                        maxSubRuns: block.compose?.maxSubRuns ?? 2,
                        maxDepth: Number(e.target.value),
                      },
                    })
                  }
                >
                  <option value={1}>1</option>
                  <option value={2}>2</option>
                </select>
              </label>
            </div>
          )}
        </div>
      </Field>

      <div className="wb-field-note">
        Select a child on the canvas to edit its agent and instructions.
      </div>
    </>
  );
}
