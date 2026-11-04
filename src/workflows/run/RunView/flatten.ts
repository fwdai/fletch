// RunView/flatten.ts — a Spec's steps (spec §5.1) as the ordered list the
// stepper, the thread, the composer's flow strip and the sidebar row render.
// Attempts key on `step_id`, so each entry is the step's id plus what those
// surfaces show about it.

import type { Spec, Step } from "../../spec";

export interface StepDesc {
  id: string;
  /** Alias into `Spec.agents`; empty for a stage no agent ran. */
  agentAlias: string;
  goal: string;
  /** The gate kind that marks this step done (spec §9), for the hover card. */
  gate?: string;
  /** Index of the top-level block this step came from, and its position inside
   *  it (0 for a plain step). Together with an attempt's `iteration` they give
   *  the thread its execution order: the retired engine ran a loop's whole body
   *  once per iteration, so inside one block the iteration outranks the body
   *  position. */
  block: number;
  position: number;
}

/** An attempt row, as far as the walker needs it to name the stages a retired
 *  run created on the fly. */
interface AttemptRef {
  step_id: string;
}

/** A block as a run's frozen launch snapshot may still hold it. The retired
 *  clone-per-step engine nested steps inside parallel, loop and orchestrate
 *  containers, and those steps have attempts in the journal that still render.
 *  Read here only, so such a run keeps its steps' identities and order; the
 *  builder and launch schema (`Spec`) stays a flat list of steps. */
interface LegacyBlock {
  step?: Step;
  parallel?: { steps?: Step[] };
  loop?: { body?: LegacyBlock[] };
  orchestrate?: { agent: string; goal: string; body?: Step[]; children?: { agent: string } };
}

/** The ids the retired engine gave the stages it ran beside the spec's own
 *  steps: a parallel block's integration merge, the agent it sent in when that
 *  merge (or an orchestrate block's) conflicted, an orchestrate block's
 *  orchestrator, and the children the orchestrator spawned on request. */
const LEGACY_MERGE_PREFIX = "__merge_";
const LEGACY_RESOLVE_PREFIX = "__resolve_";
const LEGACY_ORCHESTRATOR_PREFIX = "orchestrate-";
const LEGACY_DYNAMIC_CHILD_INFIX = "::dyn-";

class Walker {
  readonly out: StepDesc[] = [];
  constructor(private readonly attempts: AttemptRef[]) {}

  topLevel(blocks: LegacyBlock[]): void {
    blocks.forEach((b, i) => {
      this.block(b, i);
    });
  }

  /** Stage ids the run actually recorded under `prefix`, in the order their
   *  numeric suffix says, so a stage the spec never named (a dynamic child)
   *  is described only when it ran. */
  private recorded(prefix: string): string[] {
    const ids = new Set<string>();
    for (const a of this.attempts) if (a.step_id.startsWith(prefix)) ids.add(a.step_id);
    return [...ids].sort((a, b) => Number(a.slice(prefix.length)) - Number(b.slice(prefix.length)));
  }

  private push(step: Step, block: number, position: number): void {
    this.out.push({
      id: step.id,
      agentAlias: step.agent,
      goal: step.goal,
      gate: step.gate?.type,
      block,
      position,
    });
  }

  /** A stage the spec never named, described only when the run recorded an
   *  attempt for it. Returns whether it did, so the caller can keep counting. */
  private stage(id: string, agentAlias: string, goal: string, block: number, position: number) {
    if (!this.attempts.some((a) => a.step_id === id)) return false;
    this.out.push({ id, agentAlias, goal, block, position });
    return true;
  }

  private block(b: LegacyBlock, index: number): void {
    if (b.step) {
      this.push(b.step, index, 0);
    } else if (b.parallel) {
      const steps = b.parallel.steps ?? [];
      steps.forEach((s, i) => {
        this.push(s, index, i);
      });
      // The engine sent the conflicted child's own agent to resolve, then
      // completed the merge. Which child conflicted is not on the exec row, so
      // the resolver's alias is exact when the children share one and the
      // first child's otherwise; the attempt's agent record still names the
      // agent that actually ran.
      let position = steps.length;
      if (
        this.stage(
          `${LEGACY_RESOLVE_PREFIX}${index}`,
          steps[0]?.agent ?? "",
          "Resolve the conflicts the merge of the parallel work left.",
          index,
          position,
        )
      ) {
        position += 1;
      }
      this.stage(
        `${LEGACY_MERGE_PREFIX}${index}`,
        "",
        "Integrate the parallel steps' results.",
        index,
        position,
      );
    } else if (b.loop) {
      let position = 0;
      for (const inner of b.loop.body ?? []) {
        if (inner.step) this.push(inner.step, index, position++);
      }
    } else if (b.orchestrate) {
      const o = b.orchestrate;
      const orchestrator = `${LEGACY_ORCHESTRATOR_PREFIX}${index}`;
      this.out.push({
        id: orchestrator,
        agentAlias: o.agent,
        goal: o.goal,
        block: index,
        position: 0,
      });
      const body = o.body ?? [];
      body.forEach((s, i) => {
        this.push(s, index, i + 1);
      });
      const dynamic = this.recorded(`${orchestrator}${LEGACY_DYNAMIC_CHILD_INFIX}`);
      dynamic.forEach((id, i) => {
        this.out.push({
          id,
          agentAlias: o.children?.agent ?? o.agent,
          goal: "A child the orchestrator spawned on request.",
          block: index,
          position: body.length + 1 + i,
        });
      });
      // A conflicted integration of the children's work was resolved by the
      // orchestrator's own agent, after every child and before the next block.
      this.stage(
        `${LEGACY_RESOLVE_PREFIX}${index}`,
        o.agent,
        "Resolve the conflicts the merge of the children's work left.",
        index,
        body.length + 1 + dynamic.length,
      );
    }
  }
}

/** The spec's steps in document order — the rail's backbone. `attempts` lets
 *  a retired run's on-the-fly stages be named; a definition has none. */
export function flattenSteps(spec: Spec | null, attempts: AttemptRef[] = []): StepDesc[] {
  const walker = new Walker(attempts);
  walker.topLevel(spec?.workflow ?? []);
  return walker.out;
}
