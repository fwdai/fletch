// LoopContainer.tsx — a bounded loop: run the body, exit when the chosen step's
// verdict is `done`, else repeat up to `max` times (spec §6.6). Drawn as a
// bracket around a nested block sequence (replacing the v0 measured arc).

import { Icon } from "../../../components/Icon";
import type { BuilderCtx } from "../ctx";
import type { EBlock, ELoop } from "../model";
import { BlockSequence } from "./BlockSequence";
import { ContainerErrors } from "./ContainerErrors";

/** Every step in the body, flattened, as loop-exit candidates. */
function bodySteps(blocks: EBlock[]): { nid: string; label: string }[] {
  const out: { nid: string; label: string }[] = [];
  const walk = (bs: EBlock[]) => {
    for (const b of bs) {
      if (b.kind === "step") out.push({ nid: b.nid, label: b.stepId });
      else if (b.kind === "parallel")
        for (const s of b.steps) out.push({ nid: s.nid, label: s.stepId });
      else if (b.kind === "loop") walk(b.body);
      else for (const s of b.body) out.push({ nid: s.nid, label: s.stepId });
    }
  };
  walk(blocks);
  return out;
}

export function LoopContainer({ block, ctx }: { block: ELoop; ctx: BuilderCtx }) {
  const candidates = bodySteps(block.body);
  return (
    <div className="wb-cont wb-loop">
      <div className="wb-cont-h">
        <span className="wb-cont-badge">
          <Icon name="loop" size={12} /> Loop
        </span>
        <label className="wb-ctl">
          max
          <select
            className="ca-select sm"
            value={block.max}
            onChange={(e) => ctx.patchBlock(block.nid, { max: Number(e.target.value) })}
          >
            {[1, 2, 3, 4, 5, 6, 8, 10].map((n) => (
              <option key={n} value={n}>
                {n}×
              </option>
            ))}
          </select>
        </label>
        <label className="wb-ctl">
          exit when
          <select
            className="ca-select sm"
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
        </label>
        <span className="grow" />
        <button
          className="wb-step-menu tip"
          data-tip-down
          data-tip="Remove loop"
          onClick={() => ctx.removeNode(block.nid)}
        >
          <Icon name="close" />
        </button>
      </div>

      <div className="wb-loop-body">
        <BlockSequence blocks={block.body} seqNid={block.nid} ctx={ctx} />
      </div>

      <ContainerErrors errors={ctx.errorsFor(block.nid)} />
    </div>
  );
}
