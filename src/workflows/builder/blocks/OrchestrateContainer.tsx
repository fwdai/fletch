// OrchestrateContainer.tsx — a lead agent coordinating children (spec §6.6/§10).
// The orchestrator card sits above its static children; toggles expose the
// dynamic-children template, comms caps, and dynamic-composition limits. This
// renders now even though orchestrate *execution* lands in a later slice — it is
// gated behind the same validation as everything else.

import { Icon } from "../../../components/Icon";
import { STEP_COMMS } from "../../data";
import type { CommsCap } from "../../spec";
import { AgentAvatar } from "../AgentAvatar";
import type { BuilderCtx } from "../ctx";
import type { EOrchestrate } from "../model";
import { ContainerErrors } from "./ContainerErrors";
import { StepCard } from "./StepCard";

export function OrchestrateContainer({ block, ctx }: { block: EOrchestrate; ctx: BuilderCtx }) {
  const lead = ctx.resolve(block.agent);
  const child = ctx.resolve(block.children?.agent ?? null);

  const toggleComms = (cap: CommsCap) => {
    const has = block.comms.includes(cap);
    ctx.patchBlock(block.nid, {
      comms: has ? block.comms.filter((c) => c !== cap) : [...block.comms, cap],
    });
  };

  return (
    <div className="wb-cont wb-orch">
      <div className="wb-cont-h">
        <span className="wb-cont-badge">
          <Icon name="combine" size={12} /> Orchestrate
        </span>
        <label className="wb-ctl">
          join
          <select
            className="ca-select sm"
            value={block.join}
            onChange={(e) =>
              ctx.patchBlock(block.nid, { join: e.target.value as EOrchestrate["join"] })
            }
          >
            <option value="all">all</option>
            <option value="any">any</option>
          </select>
        </label>
        <label className="wb-ctl">
          integrate
          <select
            className="ca-select sm"
            value={block.integrate}
            onChange={(e) =>
              ctx.patchBlock(block.nid, { integrate: e.target.value as EOrchestrate["integrate"] })
            }
          >
            <option value="none">none</option>
            <option value="merge">merge</option>
          </select>
        </label>
        <span className="grow" />
        <button
          className="wb-step-menu tip"
          data-tip-down
          data-tip="Remove orchestrate"
          onClick={() => ctx.removeNode(block.nid)}
        >
          <Icon name="close" />
        </button>
      </div>

      <div className="wb-orch-lead">
        <button
          className="wb-step-agent"
          onClick={(e) => ctx.openAgent(block.nid, "orchestrator", e)}
        >
          {lead ? (
            <AgentAvatar
              custom={lead.custom}
              slug={lead.providerId}
              short={lead.short}
              hue={lead.hue}
              size={26}
            />
          ) : (
            <span className="wb-step-mono empty">
              <Icon name="plus" size={12} />
            </span>
          )}
          <span className="wb-step-agent-text">
            <div className={`wb-an ${lead ? "" : "empty"}`}>
              {lead ? lead.name : "Choose orchestrator"}
            </div>
            {lead && (
              <div className="wb-am">
                {lead.custom ? `${lead.baseLabel} · ${lead.model}` : lead.model}
              </div>
            )}
          </span>
        </button>
        <textarea
          className="wb-step-goal"
          placeholder="How should the orchestrator coordinate its children?"
          value={block.goal}
          onChange={(e) => ctx.patchBlock(block.nid, { goal: e.target.value })}
        />
        <div className="wb-comms">
          <span className="wb-comms-l">Children may</span>
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
      </div>

      <div className="wb-orch-children">
        <div className="wb-sub-h">Static children</div>
        <div className="wb-parallel-body">
          {block.body.map((s, i) => (
            <StepCard
              key={s.nid}
              step={s}
              ctx={ctx}
              indexLabel={`${i + 1}`}
              canRemove
              role="child"
            />
          ))}
          <button className="wb-add sm" onClick={() => ctx.addStepToContainer(block.nid)}>
            <span className="wb-add-ic">
              <Icon name="plus" />
            </span>
            <span className="wb-add-l">Add child</span>
          </button>
        </div>
      </div>

      <div className="wb-orch-opts">
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
          Dynamic children
        </label>
        {block.children && (
          <div className="wb-opt-row">
            <button className="wb-adv-sel" onClick={(e) => ctx.openAgent(block.nid, "child", e)}>
              {child ? child.name : "child agent…"}
            </button>
            <label className="wb-ctl">
              max
              <input
                className="ca-input sm"
                type="number"
                min={1}
                style={{ width: 52 }}
                value={block.children.max}
                onChange={(e) =>
                  ctx.patchBlock(block.nid, {
                    children: { agent: block.children?.agent ?? null, max: Number(e.target.value) },
                  })
                }
              />
            </label>
          </div>
        )}

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
          Allow sub-workflows
        </label>
        {block.compose && (
          <div className="wb-opt-row">
            <label className="wb-ctl">
              sub-runs
              <input
                className="ca-input sm"
                type="number"
                min={1}
                style={{ width: 52 }}
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
            <label className="wb-ctl">
              depth
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

      <ContainerErrors errors={ctx.errorsFor(block.nid)} />
    </div>
  );
}
