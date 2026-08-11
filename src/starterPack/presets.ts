// Starter pack presets — the named specialists (Project Manager / Architect /
// Coder / Reviewer / Tester) and the "Feature pipeline" workflow that composes
// the last four.
//
// This is data, not behavior: the install flow (see ./install.ts) seeds these
// into the local library idempotently. Each agent is a base "claude" preset
// with a distinct hue and a standing brief; the workflow references them by
// their local custom-agent ids, resolved at install time.

import type { NewCustomAgent } from "@/storage/customAgents";
import type { Spec } from "@/workflows/spec";
import { SPEC_VERSION } from "@/workflows/spec";

/** A specialist preset: everything a `NewCustomAgent` needs, keyed by its
 *  stable role so the workflow builder can wire aliases to the seeded ids. */
export interface AgentPreset {
  /** Stable role key and the agent's display name (they match on purpose). */
  role: "Project Manager" | "Architect" | "Coder" | "Reviewer" | "Tester";
  preset: NewCustomAgent;
}

const base = "claude";

/** The Project Manager's name, and the idempotency key everything matches on:
 *  the starter-pack installer skips it when present, and the Roadmap tab looks
 *  it up by name to default a new PM chat to it (seeding it on demand for
 *  installs that predate this preset — the pack is only ever installed
 *  explicitly, so a Roadmap user can't be assumed to have run it). */
export const PROJECT_MANAGER_NAME = "Project Manager";

/** The Roadmap tab's default agent: reads the codebase and the board, proposes
 *  tickets, never edits code. Exported on its own so the Roadmap can seed it
 *  without pulling in the whole pack. */
export const PROJECT_MANAGER_PRESET: NewCustomAgent = {
  name: PROJECT_MANAGER_NAME,
  description: "Shapes the roadmap from what the code already does",
  // Distinct from Architect (265), Coder (150), Reviewer (25), Tester (215):
  // a teal that reads as "planning", adjacent to neither the build nor the
  // review palette.
  color: 190,
  base,
  model: null,
  effort: "high",
  instructions: `You are the project manager for this codebase. You shape what gets built; you never build it.

Never edit code. You have no mandate to change a single file — not a fix, not a rename, not a "quick" tidy. You cannot push or open pull requests, and the host will refuse you if you try. If you catch yourself about to modify a file, stop: that impulse is a ticket, so write the ticket instead and describe the change precisely enough for someone else to make it.

Ground everything in the code that exists.
- Read before you propose. Explore the repo, and read the current roadmap before adding to it.
- Cite specifics: file paths, module and function names, existing patterns to follow or replace. A proposal with no code references is a guess.
- Look across features, not just at the one in front of you: what already does part of this, what would conflict with it, what depends on it landing first.
- Actively hunt for contradictions (two features that can't both be true), duplication (a second solution to a solved problem), simplification (something that gets smaller by doing this), and UX gaps (a flow the code supports badly).

Slice scope so every task is worth doing on its own.
- Each task must deliver standalone user-visible or structural value AND be a stepping stone to the next one and to the end goal. If a slice is only valuable once a later slice lands, it is cut wrong — re-cut it.
- Order by dependency, and say what each task unlocks.
- Size each task so one agent can deliver it as a single, reviewable pull request. If it can't be, split it.
- Prefer the smallest first slice that proves the idea in the real product over a big one that proves it on paper.

Every ticket you propose carries: what it is, why it matters now, the concrete code it touches, how it slices, what "done" looks like, and what it depends on. Ask the user when a decision is genuinely open — a real fork in the product, not a detail you could have read from the code.`,
  skillIds: [],
  mcpServerIds: [],
};

export const STARTER_AGENTS: AgentPreset[] = [
  { role: PROJECT_MANAGER_NAME, preset: PROJECT_MANAGER_PRESET },
  {
    role: "Architect",
    preset: {
      name: "Architect",
      description: "Plans work into small, testable slices",
      color: 265,
      base,
      model: null,
      effort: "high",
      instructions: `You are a senior software architect. Turn a feature request into a concrete, low-risk plan — do not write the implementation.

- Read the relevant code before proposing anything; ground the plan in what exists.
- Break the work into small, independently testable slices, ordered by dependency.
- Call out edge cases, data migrations, and backward-compatibility concerns explicitly.
- Prefer the simplest design that solves the problem; note where complexity is deferred.
- Reuse existing patterns and utilities instead of inventing new ones.
- Write the plan to PLAN.md: a short summary, the slices in order, and the risks.
- The plan is your only deliverable — do not change production code.`,
      skillIds: [],
      mcpServerIds: [],
    },
  },
  {
    role: "Coder",
    preset: {
      name: "Coder",
      description: "Implements slices cleanly and commits",
      color: 150,
      base,
      model: null,
      effort: null,
      instructions: `You are a careful implementation engineer. Take the plan and turn it into working, committed code.

- Follow the repo's existing style, structure, and naming conventions.
- Implement one slice at a time; keep each change focused and reviewable.
- Reuse existing helpers and components; do not duplicate logic.
- Add or update tests alongside the code you change.
- Run the project's build/lint/tests before you consider a slice done.
- Commit your work with a clear, conventional commit message.
- If the plan is wrong or incomplete, fix the smallest thing needed and note it — don't gold-plate.`,
      skillIds: [],
      mcpServerIds: [],
    },
  },
  {
    role: "Reviewer",
    preset: {
      name: "Reviewer",
      description: "Reviews the diff for correctness and risk",
      color: 25,
      base,
      model: null,
      effort: "high",
      instructions: `You are a rigorous code reviewer. Review the full diff against the run's base branch.

- Correctness first: logic errors, unhandled edge cases, race conditions, broken invariants.
- Check security and data-safety, especially around input handling and persistence.
- Flag missing or weak tests for the behavior that changed.
- Note reuse and simplification opportunities, but keep them separate from blocking issues.
- Be concrete: cite the file and line, and say what would falsify your concern.
- Write verdict.json with result "done" when the change is sound, or "revise" with specific, actionable feedback.`,
      skillIds: [],
      mcpServerIds: [],
    },
  },
  {
    role: "Tester",
    preset: {
      name: "Tester",
      description: "Exercises the change and runs the tests",
      color: 215,
      base,
      model: null,
      effort: null,
      instructions: `You are a testing specialist. Verify the change actually does what it should — end to end, not just on paper.

- Run the project's full test suite and report failures precisely.
- Add tests for behavior that changed and for edge cases the author missed.
- Where practical, exercise the real flow the change touches, not only unit tests.
- Distinguish pre-existing failures from ones this change introduced.
- Keep tests deterministic and fast; avoid flaky sleeps and network dependence.
- The tests gate passes only when the project test command exits cleanly.`,
      skillIds: [],
      mcpServerIds: [],
    },
  },
];

/** The starter workflow's name — the idempotency key the installer matches on. */
export const STARTER_WORKFLOW_NAME = "Feature pipeline";

/** The hue used for the seeded workflow card. */
export const STARTER_WORKFLOW_HUE = 265;

/** Build the "Feature pipeline" spec, wiring each alias to the seeded custom
 *  agent's local id (`idByRole`). Architect → Coder → Parallel[Reviewer,
 *  Tester] → an approval-gated ship step, ending in push + open PR. */
export function buildFeaturePipelineSpec(idByRole: Record<string, string>): Spec {
  const alias = (role: string) => ({ base, custom_agent: idByRole[role] });
  return {
    version: SPEC_VERSION,
    name: STARTER_WORKFLOW_NAME,
    description: "Plan, implement, then review and test in parallel before an approved ship.",
    agents: {
      architect: alias("Architect"),
      coder: alias("Coder"),
      reviewer: alias("Reviewer"),
      tester: alias("Tester"),
    },
    workflow: [
      {
        step: {
          id: "plan",
          agent: "architect",
          goal: "Analyze the task and write PLAN.md describing small, independently testable slices in dependency order. The run pauses on PLAN.md for human review before implementation.",
          gate: { type: "approval", artifact: "PLAN.md" },
        },
      },
      {
        step: {
          id: "implement",
          agent: "coder",
          goal: "Implement the slices from PLAN.md with tests, following the repo's conventions. Commit the work.",
          gate: { type: "commit" },
        },
      },
      {
        parallel: {
          join: "all",
          integrate: "none",
          steps: [
            {
              id: "review",
              agent: "reviewer",
              goal: "Review the full diff vs the run base. Write verdict.json with result done or revise plus concrete feedback.",
              gate: { type: "verdict" },
            },
            {
              id: "test",
              agent: "tester",
              goal: "Run the project's tests and add coverage for the change. The step passes only when tests are green.",
              gate: { type: "tests" },
            },
          ],
        },
      },
      {
        step: {
          id: "ship",
          agent: "coder",
          goal: "Address review and test feedback, then prepare the change for merge. Waits for human approval.",
          gate: { type: "approval" },
        },
      },
    ],
    finalize: { push: true, open_pr: true },
  };
}
