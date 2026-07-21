// LoopContainer.tsx — a bounded loop: run the body, exit when the chosen step's
// verdict is `done`, else repeat up to `max` times (spec §6.6). Drawn as an
// accent-tinted group around a nested vertical sequence; max/exit are edited in
// the inspector when the container is selected.

import { Icon } from "../../../components/Icon";
import type { BuilderCtx } from "../ctx";
import { loopExitCandidates } from "../edits";
import type { ELoop } from "../model";
import { BlockSequence } from "./BlockSequence";

export function LoopContainer({
  block,
  ctx,
  indexLabel,
}: {
  block: ELoop;
  ctx: BuilderCtx;
  indexLabel?: string;
}) {
  const errors = ctx.errorsFor(block.nid);
  const selected = ctx.selectedNid === block.nid;
  const exit = block.untilNid
    ? loopExitCandidates(block.body).find((c) => c.nid === block.untilNid)
    : undefined;

  return (
    <div
      className={`wb-cont wb-loop ${selected ? "sel" : ""} ${errors ? "has-err" : ""}`}
      onClick={() => ctx.select(block.nid)}
    >
      <div className="wb-cont-h">
        {indexLabel && <span className="wb-step-idx">{indexLabel}</span>}
        <span className="wb-cont-badge loop">
          <Icon name="loop" size={12} /> Loop
        </span>
        <span className="wb-cont-sum">
          up to {block.max}× · exits when {exit ? <b>{exit.label}</b> : "…"} is done
        </span>
        {errors && (
          <span className="wb-chip err">
            <Icon name="close" /> {errors.length}
          </span>
        )}
        <button
          className="wb-step-menu tip"
          data-tip-down
          data-tip="Remove loop"
          onClick={(e) => {
            e.stopPropagation();
            ctx.removeNode(block.nid);
          }}
        >
          <Icon name="close" />
        </button>
      </div>

      <div className="wb-cont-body wb-loop-body" onClick={(e) => e.stopPropagation()}>
        <BlockSequence blocks={block.body} seqNid={block.nid} ctx={ctx} />
      </div>
    </div>
  );
}
