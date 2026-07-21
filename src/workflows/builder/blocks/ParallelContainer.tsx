// ParallelContainer.tsx — a fan-out stage: children run at once, then join.
// Collapsed chrome on the canvas (a labelled container with a branch grid);
// join/integrate/max-concurrency are edited in the inspector when selected.

import { Icon } from "../../../components/Icon";
import type { BuilderCtx } from "../ctx";
import type { EParallel } from "../model";
import { StepCard } from "./StepCard";

export function ParallelContainer({
  block,
  ctx,
  indexLabel,
}: {
  block: EParallel;
  ctx: BuilderCtx;
  indexLabel?: string;
}) {
  const errors = ctx.errorsFor(block.nid);
  const selected = ctx.selectedNid === block.nid;

  return (
    <div
      className={`wb-cont wb-parallel ${selected ? "sel" : ""} ${errors ? "has-err" : ""}`}
      onClick={() => ctx.select(block.nid)}
    >
      <div className="wb-cont-h">
        {indexLabel && <span className="wb-step-idx">{indexLabel}</span>}
        <span className="wb-cont-badge">
          <Icon name="layers" size={12} /> Parallel
        </span>
        <span className="wb-cont-sum">
          join {block.join}
          {block.maxConcurrent != null ? ` · ${block.maxConcurrent} at once` : ""} ·{" "}
          {block.steps.length} branch{block.steps.length === 1 ? "" : "es"}
        </span>
        {errors && (
          <span className="wb-chip err">
            <Icon name="close" /> {errors.length}
          </span>
        )}
        <button
          className="wb-step-menu tip"
          data-tip-down
          data-tip="Remove parallel"
          onClick={(e) => {
            e.stopPropagation();
            ctx.removeNode(block.nid);
          }}
        >
          <Icon name="close" />
        </button>
      </div>

      {/* Clicks inside the body select the child card, not this container. */}
      <div className="wb-cont-body wb-branches" onClick={(e) => e.stopPropagation()}>
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
        <button
          className="wb-add sm"
          onClick={(e) => {
            e.stopPropagation();
            ctx.addStepToContainer(block.nid);
          }}
        >
          <span className="wb-add-ic">
            <Icon name="plus" />
          </span>
          <span className="wb-add-l">Add branch</span>
        </button>
      </div>
    </div>
  );
}
