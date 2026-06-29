// run/prompt.ts — the per-step spawn prompt.
//
// Each step is a fresh agent with no memory of prior steps; this prompt is the
// framing the orchestrator injects: the step's place in the workflow, its goal,
// where to find prior context (./.quorum/), and the protocol for signalling done
// / looping in a way the engine can detect deterministically. The agent's role
// (its standing instructions) is injected separately at spawn.

import type { WorkflowStep } from "../storage";

/** The handoff dir agents read prior notes from and write their own to. */
export const HANDOFF_DIR = ".quorum";

export function buildStepPrompt(
  step: WorkflowStep,
  index: number,
  total: number,
  ctx: { workflowName: string; task: string },
): string {
  const noteFile = `${HANDOFF_DIR}/${step.id}.md`;
  const lines: string[] = [
    `You are step ${index + 1} of ${total} in the "${ctx.workflowName}" workflow.`,
    `Overall task: ${ctx.task}`,
    "",
    "What this step should accomplish:",
    step.goal?.trim() || "(no specific goal given — use your role's judgment)",
    "",
    `Prior steps left context in the ./${HANDOFF_DIR}/ directory — read it before starting.`,
    `When you finish, write a brief handoff note for the next step to ./${noteFile}.`,
  ];

  // Advance-mode protocol — tell the agent how to satisfy the gate the builder set.
  switch (step.advance) {
    case "commit":
      lines.push("Commit your work with git before you finish.");
      break;
    case "tests":
      lines.push("Make sure the project's tests pass before you finish.");
      break;
    case "artifact":
      if (step.artifact) lines.push(`Produce the file: ${step.artifact}.`);
      break;
    case "approval":
      lines.push("When done, stop and wait — a human will review before the next step runs.");
      break;
    case "signal":
    default:
      break;
  }

  // Loop protocol — detected by the presence of a marker file, so the engine
  // never has to parse free-form text.
  if (step.loop) {
    lines.push(
      `If ${step.loop.when}, create the file ./${HANDOFF_DIR}/${step.id}.loop ` +
        `(its presence sends work back to an earlier step). Otherwise do not create it.`,
    );
  }

  return lines.join("\n");
}

/** Relative path of a step's loop-decision marker. */
export const loopMarker = (stepId: string) => `${HANDOFF_DIR}/${stepId}.loop`;
