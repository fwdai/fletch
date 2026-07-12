// ParallelContainer.tsx — a fan-out stage: children run at once, then join.
// Rendered as a labelled container wrapping a vertical stack of step cards.

import { Icon } from "../../../components/Icon";
import type { BuilderCtx } from "../ctx";
import type { EParallel } from "../model";
import { ContainerErrors } from "./ContainerErrors";
import { StepCard } from "./StepCard";

export function ParallelContainer({ block, ctx }: { block: EParallel; ctx: BuilderCtx }) {
  return (
    <div className="wb-cont wb-parallel">
      <div className="wb-cont-h">
        <span className="wb-cont-badge">
          <Icon name="layers" size={12} /> Parallel
        </span>
        <label className="wb-ctl">
          join
          <select
            className="ca-select sm"
            value={block.join}
            onChange={(e) =>
              ctx.patchBlock(block.nid, { join: e.target.value as EParallel["join"] })
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
              ctx.patchBlock(block.nid, { integrate: e.target.value as EParallel["integrate"] })
            }
          >
            <option value="none">none</option>
            <option value="merge">merge</option>
          </select>
        </label>
        <label className="wb-ctl">
          max at once
          <input
            className="ca-input sm"
            type="number"
            min={1}
            style={{ width: 52 }}
            placeholder="all"
            value={block.maxConcurrent ?? ""}
            onChange={(e) =>
              ctx.patchBlock(block.nid, {
                maxConcurrent: e.target.value.trim() === "" ? null : Number(e.target.value),
              })
            }
          />
        </label>
        <span className="grow" />
        <button
          className="wb-step-menu tip"
          data-tip-down
          data-tip="Remove parallel"
          onClick={() => ctx.removeNode(block.nid)}
        >
          <Icon name="close" />
        </button>
      </div>

      <div className="wb-cont-body wb-parallel-body">
        {block.steps.map((s, i) => (
          <StepCard
            key={s.nid}
            step={s}
            ctx={ctx}
            indexLabel={`${i + 1}`}
            canRemove={block.steps.length > 1}
            role="child"
          />
        ))}
        <button className="wb-add sm" onClick={() => ctx.addStepToContainer(block.nid)}>
          <span className="wb-add-ic">
            <Icon name="plus" />
          </span>
          <span className="wb-add-l">Add branch</span>
        </button>
      </div>

      <ContainerErrors errors={ctx.errorsFor(block.nid)} />
    </div>
  );
}
