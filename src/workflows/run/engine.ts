// run/engine.ts — the orchestration engine.
//
// Drives a run forward one step at a time: spawn the step's agent (forked from
// the previous step's HEAD), ferry handoff notes in, send the step prompt, wait
// for the turn to end, evaluate the advance gate, boundary-commit, then loop or
// advance — finishing with a push + PR. State is persisted on every transition
// so a run resumes after an app restart.

import { type AgentStatus, api, onAgentStatus } from "../../api";
import { sendWhenAgentReady, snapshotAgentDeliverables } from "../../helpers";
import type { CustomAgent } from "../../storage/customAgents";
import { useAppStore } from "../../store";
import type { Workflow, WorkflowStep } from "../storage";
import type { StepRef } from "./gitOps";
import * as git from "./gitOps";
import { buildStepPrompt, loopMarker } from "./prompt";
import { getRun, listRuns, saveRun, saveRunStep } from "./storage";
import type { WorkflowRun, WorkflowRunStep } from "./types";
import { RUN_TERMINAL } from "./types";

// ── change notification (the monitor refreshes on these) ──────────────────
const listeners = new Set<() => void>();
export function subscribeRuns(cb: () => void): () => void {
  listeners.add(cb);
  return () => listeners.delete(cb);
}
function notify() {
  for (const cb of listeners) cb();
}

// In-flight guard so a run is only driven by one loop at a time.
const driving = new Set<string>();
// Runs the user asked to stop; the drive loop bails at its next checkpoint.
const canceled = new Set<string>();

const uid = (p: string) => `${p}-${Date.now()}-${Math.round(Math.random() * 1e6)}`;
const slug = (s: string) =>
  s
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 32) || "run";

async function persistRun(run: WorkflowRun): Promise<WorkflowRun> {
  const next = { ...run, updated_at: Date.now() };
  await saveRun(next);
  notify();
  return next;
}
async function persistStep(step: WorkflowRunStep): Promise<WorkflowRunStep> {
  await saveRunStep(step);
  notify();
  return step;
}

/** Resolve a step's checkout ref (subdir) from the live workspace. */
function refFor(agentId: string): StepRef {
  const ws = useAppStore.getState().workspace;
  const agent = ws?.agents.find((a) => a.id === agentId);
  return { agentId, subdir: agent?.repos[0]?.subdir ?? "repo" };
}

/** Spawn parameters for a step's agent (custom agent → its base/model/brief). */
function stepSpawnParams(step: WorkflowStep, customAgents: CustomAgent[]) {
  const ca = customAgents.find((a) => a.id === step.agent);
  if (ca) {
    // Same by-value snapshot semantics as the draft spawn path: the step's
    // agent gets its skills and deliverable MCP servers resolved at spawn.
    const { skills, mcpServers } = snapshotAgentDeliverables(useAppStore.getState(), ca, ca.base);
    return {
      provider: ca.base,
      model: ca.model ?? undefined,
      instructions: ca.instructions?.trim() ? ca.instructions : undefined,
      customAgentId: ca.id,
      skills,
      mcpServers,
    };
  }
  return {
    provider: step.agent ?? "claude",
    model: undefined,
    instructions: undefined,
    customAgentId: undefined,
    skills: undefined,
    mcpServers: undefined,
  };
}

/** Resolve when `agentId` reaches one of `targets` (optionally only after it has
 *  been seen running), or terminates with error/stopped. */
function awaitStatus(
  agentId: string,
  targets: AgentStatus[],
  opts: { afterRunning?: boolean } = {},
): Promise<AgentStatus> {
  return new Promise((resolve) => {
    let sawRunning = false;
    let off: (() => void) | null = null;
    let done = false;
    const finish = (status: AgentStatus) => {
      if (done) return;
      done = true;
      off?.();
      resolve(status);
    };
    onAgentStatus((e) => {
      if (e.agent_id !== agentId) return;
      if (e.status === "running") sawRunning = true;
      const hit = targets.includes(e.status) && (!opts.afterRunning || sawRunning);
      if (hit || e.status === "error" || e.status === "stopped") finish(e.status);
    }).then((u) => {
      off = u;
      if (done) u();
    });
  });
}

type StepOutcome =
  | { kind: "done"; ref: StepRef; head: string }
  | { kind: "error"; ref: StepRef }
  | { kind: "stopped"; ref: StepRef }
  | { kind: "blocked"; ref: StepRef }
  | { kind: "awaiting_approval"; ref: StepRef };

/** Evaluate a step's advance gate at turn-end. */
async function evaluateGate(
  step: WorkflowStep,
  ref: StepRef,
  headStart: string,
): Promise<"done" | "blocked" | "awaiting_approval"> {
  switch (step.advance) {
    case "approval":
      return "awaiting_approval";
    case "commit": {
      const { head } = await git.headSha(ref);
      return head !== headStart ? "done" : "blocked";
    }
    case "artifact": {
      if (!step.artifact) return "done";
      const { exists } = await git.fileExists(ref, step.artifact);
      return exists ? "done" : "blocked";
    }
    // `tests` runs the project command — deferred; treat as signals-done for now.
    case "tests":
    case "signal":
    default:
      return "done";
  }
}

async function executeStep(
  run: WorkflowRun,
  step: WorkflowStep,
  index: number,
  iteration: number,
  prevHead: string | undefined,
  prevRef: StepRef | null,
): Promise<StepOutcome> {
  const total = run.steps_snapshot.length;
  const customAgents = useAppStore.getState().customAgents;
  const sp = stepSpawnParams(step, customAgents);

  const rec = await api.spawnAgent(
    "custom",
    run.repo_path,
    sp.provider,
    undefined,
    undefined,
    sp.model,
    sp.instructions,
    sp.customAgentId,
    prevHead,
    sp.skills,
    sp.mcpServers,
  );
  const ref: StepRef = { agentId: rec.id, subdir: rec.repos[0]?.subdir ?? "repo" };

  let row: WorkflowRunStep = {
    id: uid("wfstep"),
    run_id: run.id,
    step_id: step.id,
    iteration,
    agent_id: rec.id,
    status: "running",
    advance_mode: step.advance,
    head_start: null,
    head_end: null,
    summary: null,
    started_at: Date.now(),
    ended_at: null,
  };
  await persistStep(row);

  // Ready → ferry prior notes into this checkout → snapshot the fork point.
  await awaitStatus(rec.id, ["idle"]);
  if (canceled.has(run.id)) {
    await persistStep({ ...row, status: "error", ended_at: Date.now(), summary: "stopped" });
    return { kind: "stopped", ref };
  }
  if (prevRef) await git.ferryNotes(prevRef, ref);
  const { head: headStart } = await git.headSha(ref);
  row = await persistStep({ ...row, head_start: headStart });

  // Send the step prompt and wait for the turn to finish.
  const turnEnd = awaitStatus(rec.id, ["idle"], { afterRunning: true });
  await sendWhenAgentReady(() =>
    api.sendUserMessage(
      rec.id,
      crypto.randomUUID(),
      buildStepPrompt(step, index, total, { workflowName: run.name, task: run.task }),
    ),
  );
  const endStatus = await turnEnd;

  // User stopped the agent (or canceled the run) mid-turn — bail without
  // advancing; driveRun marks the whole run canceled.
  if (endStatus === "stopped" || canceled.has(run.id)) {
    await persistStep({ ...row, status: "error", ended_at: Date.now(), summary: "stopped" });
    return { kind: "stopped", ref };
  }
  if (endStatus === "error") {
    await persistStep({ ...row, status: "error", ended_at: Date.now(), summary: "agent error" });
    return { kind: "error", ref };
  }

  const gate = await evaluateGate(step, ref, headStart);
  if (gate !== "done") {
    await persistStep({ ...row, status: gate, ended_at: Date.now() });
    return gate === "awaiting_approval"
      ? { kind: "awaiting_approval", ref }
      : { kind: "blocked", ref };
  }

  const { head } = await git.boundaryCommit(ref, `wf(${run.name}): step ${index + 1} · ${step.id}`);
  await persistStep({ ...row, status: "done", head_end: head, ended_at: Date.now() });
  return { kind: "done", ref, head };
}

/** Drive a run from its persisted position to completion or a pause. */
export async function driveRun(runId: string): Promise<void> {
  if (driving.has(runId)) return;
  driving.add(runId);
  canceled.delete(runId); // a fresh drive clears any stale stop request
  try {
    const data = await getRun(runId);
    if (!data || RUN_TERMINAL.has(data.run.status)) return;
    let run = data.run;
    const steps = run.steps_snapshot;

    // Resume cursor: last completed execution sets the fork point; current_step_id
    // / current_iter say where to (re)start. A step interrupted mid-flight is
    // simply re-run from the last committed HEAD.
    const doneRows = data.steps.filter((s) => s.status === "done");
    const lastDone = doneRows[doneRows.length - 1];
    // Step 1 forks from the run's base (the chosen base branch); later steps from
    // the previous step's HEAD.
    let prevHead: string | undefined = lastDone?.head_end ?? (run.base_sha || undefined);
    let prevRef: StepRef | null = lastDone?.agent_id ? refFor(lastDone.agent_id) : null;
    let index = Math.max(
      0,
      steps.findIndex((s) => s.id === run.current_step_id),
    );
    let iteration = run.current_iter ?? 0;

    run = await persistRun({ ...run, status: "running" });

    while (index < steps.length) {
      const step = steps[index];
      run = await persistRun({ ...run, current_step_id: step.id, current_iter: iteration });

      const outcome = await executeStep(run, step, index, iteration, prevHead, prevRef);
      if (outcome.kind === "stopped") {
        await persistRun({ ...run, status: "canceled", current_step_id: null });
        return;
      }
      if (outcome.kind === "error") {
        await persistRun({ ...run, status: "failed" });
        return;
      }
      if (outcome.kind === "blocked" || outcome.kind === "awaiting_approval") {
        await persistRun({ ...run, status: "paused" });
        return;
      }

      prevHead = outcome.head;
      prevRef = outcome.ref;

      // Loop back if this step asked to and the cap isn't reached.
      if (step.loop) {
        const target = steps.findIndex((s) => s.id === step.loop!.to);
        const { exists } = await git.fileExists(outcome.ref, loopMarker(step.id));
        if (exists && target >= 0 && iteration < step.loop.max) {
          index = target;
          iteration += 1;
          continue;
        }
      }
      index += 1;
      iteration = 0;
    }

    // All steps done → push the final HEAD to the run branch + open a PR.
    // The PR targets the branch the run forked from (empty = the repo default).
    if (prevRef) {
      await git.finalize(prevRef, {
        branch: run.branch,
        baseBranch: run.base_sha || undefined,
        title: `${run.name}: ${run.task}`.slice(0, 120),
        body: run.task,
      });
    }
    await persistRun({ ...run, status: "done", current_step_id: null });
  } catch (e) {
    const data = await getRun(runId);
    if (data && !RUN_TERMINAL.has(data.run.status)) {
      await persistRun({ ...data.run, status: "failed" });
    }
    useAppStore.getState().setLastError?.(`Workflow run failed: ${e}`);
  } finally {
    driving.delete(runId);
  }
}

/** Launch a workflow on a task: create the run, exclude `.quorum/`, and drive. */
export async function launchRun(
  workflow: Workflow,
  opts: { task: string; projectId: string; repoPath: string; baseBranch?: string },
): Promise<WorkflowRun> {
  const now = Date.now();
  const id = uid("wfrun");
  const run: WorkflowRun = {
    id,
    workflow_id: workflow.id,
    name: workflow.name,
    steps_snapshot: JSON.parse(JSON.stringify(workflow.steps)),
    task: opts.task,
    project_id: opts.projectId,
    repo_path: opts.repoPath,
    run_dir: "",
    branch: `wf/${slug(workflow.name)}-${id.slice(-6)}`,
    // Where step 1 forks from (a branch name works as a commit-ish); later steps
    // fork from the previous step's HEAD. Empty = the repo's current branch.
    base_sha: opts.baseBranch ?? "",
    status: "pending",
    current_step_id: workflow.steps[0]?.id ?? null,
    current_iter: 0,
    created_at: now,
    updated_at: now,
  };
  await persistRun(run);
  await git.prepareRepo(opts.repoPath);
  void driveRun(id);
  return run;
}

/** Approve the step a run is paused on, commit it, and continue. */
export async function approveStep(runId: string): Promise<void> {
  const data = await getRun(runId);
  if (!data) return;
  const row = data.steps.find((s) => s.status === "awaiting_approval");
  if (!row?.agent_id) return;
  const ref = refFor(row.agent_id);
  const { head } = await git.boundaryCommit(
    ref,
    `wf(${data.run.name}): step ${row.step_id} (approved)`,
  );
  await persistStep({ ...row, status: "done", head_end: head, ended_at: Date.now() });
  const steps = data.run.steps_snapshot;
  const idx = steps.findIndex((s) => s.id === row.step_id);
  await persistRun({
    ...data.run,
    status: "running",
    current_step_id: steps[idx + 1]?.id ?? null,
    current_iter: 0,
  });
  void driveRun(runId);
}

/** Stop a run the way an agent is stopped: halt the active step's agent and mark
 *  the run canceled. The drive loop sees the agent stop (turn-end → "stopped")
 *  and bails at its next checkpoint; setting the status here also covers a run
 *  with no live drive loop. */
export async function cancelRun(runId: string): Promise<void> {
  const data = await getRun(runId);
  if (!data || RUN_TERMINAL.has(data.run.status)) return;
  canceled.add(runId);
  const running = data.steps.find((s) => s.status === "running");
  if (running?.agent_id) {
    try {
      await api.stopAgent(running.agent_id);
    } catch {
      /* agent may already be gone; the status write below still cancels the run */
    }
  }
  await persistRun({ ...data.run, status: "canceled", current_step_id: null });
}

/** On app start, continue any runs that were mid-flight (not paused/terminal). */
export async function resumeActiveRuns(): Promise<void> {
  const runs = await listRuns();
  for (const r of runs) {
    if (r.status === "running" || r.status === "pending") void driveRun(r.id);
  }
}
