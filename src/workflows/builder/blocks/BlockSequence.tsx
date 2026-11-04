// BlockSequence.tsx — the workflow's steps as a vertical pipeline: a connector
// between each pair (hover it to insert a step there) and a trailing "Add step".

import { Fragment } from "react";
import { Icon } from "../../../components/Icon";
import type { BuilderCtx } from "../ctx";
import type { EStep } from "../model";
import { StepCard } from "./StepCard";

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

export function BlockSequence({ steps, ctx }: { steps: EStep[]; ctx: BuilderCtx }) {
  // A workflow needs at least one step, so the last one stays.
  const canRemove = steps.length > 1;

  return (
    <div className="wb-seq">
      {steps.map((s, i) => (
        <Fragment key={s.nid}>
          {i > 0 && (
            <div className="wb-conn">
              <button
                className="wb-insert tip"
                data-tip="Insert a step here"
                onClick={() => ctx.addStep(i)}
              >
                <Icon name="plus" />
              </button>
            </div>
          )}
          <StepCard step={s} ctx={ctx} indexLabel={pad2(i + 1)} canRemove={canRemove} />
        </Fragment>
      ))}

      <button className="wb-add" onClick={() => ctx.addStep()}>
        <span className="wb-add-ic">
          <Icon name="plus" />
        </span>
        <span className="wb-add-l">Add step</span>
      </button>
    </div>
  );
}
