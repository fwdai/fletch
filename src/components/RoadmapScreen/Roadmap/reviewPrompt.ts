// The prompt "Fix review feedback" hands an agent, composed from the item and
// its PR's unresolved review threads.
//
// Pure, and separate from the card, because it is the actual product of that
// button: the agent's whole understanding of the job is this text, and a card
// that composed it inline would make it untestable. The agent gets no roadmap
// tool access and no board — everything it needs to know has to be in here.
//
// Register note: this is a *draft seed*, not a delegation trigger. The
// `[app-action]` one-liners in delegation.ts work because their playbooks are
// already injected into a running agent's instructions
// (`instructions/git_actions.md`); a draft has no agent yet, so nothing would
// resolve a terse trigger. Hence prose, and hence the threads quoted in full
// rather than referenced.

import type { PrComment, RoadmapItem } from "@/api";

/** One thread, as a block the agent can act on: who said it, where, and what.
 *
 *  A thread we replied to last is annotated rather than dropped. Dropping it
 *  would hide a live disagreement; presenting it like the others invites the
 *  agent to re-argue a point already made (the same distinction readiness.ts
 *  draws between "unaddressed" and "disputed"). */
function threadBlock(thread: PrComment, index: number): string {
  const where = thread.path
    ? `${thread.path}${thread.line != null ? `:${thread.line}` : ""}`
    : "no file — the line it was on is gone";
  const parts = [`${index}. @${thread.author} — ${where}`];
  if (thread.we_replied_last) {
    parts.push("   (we answered this one last — read the exchange before replying again)");
  }
  // Indented so a multi-line comment stays visibly one thread's body.
  for (const line of thread.body.trim().split("\n")) {
    parts.push(`   ${line}`.trimEnd());
  }
  return parts.join("\n");
}

/** The seed prompt for a fix agent, or `null` when there is nothing to fix.
 *
 *  `null` is the action's gate: no unresolved threads means no work to hand over,
 *  so the card offers no button rather than spawning an agent with an empty
 *  brief. Both the button and this function read the same emptiness, so they
 *  cannot disagree.
 *
 *  The header is the item's identity only (code + title) — not its full brief.
 *  The job here is not "build the feature", it is "answer these threads on this
 *  PR"; the why/acceptance-criteria belong to the run that already did the
 *  building, and repeating them invites a rewrite. */
export function reviewFeedbackPrompt(
  item: RoadmapItem,
  threads: readonly PrComment[],
): string | null {
  if (threads.length === 0) return null;
  const pr = item.pr_number != null ? `PR #${item.pr_number}` : "its pull request";
  const count =
    threads.length === 1
      ? "1 unresolved review thread"
      : `${threads.length} unresolved review threads`;
  const lines = [
    `${item.code}: ${item.title}`,
    "",
    `${pr} has ${count}:`,
    "",
    ...threads.flatMap((t, i) => [threadBlock(t, i + 1), ""]),
    "Address each thread on this PR's branch; push when green.",
  ];
  if (item.pr_url) lines.push("", item.pr_url);
  return lines.join("\n");
}
