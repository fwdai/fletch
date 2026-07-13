// RunView/PausedBanner.tsx — the paused/failed banner (spec §14.2). Every state a
// run can rest in names its cause in plain language and offers its action. Actions
// land as their slices do: approve (S4), retry/resume (S4), while answer (S10) and
// conflict resolution (S9) render as clearly-labelled "arrives next" affordances
// rather than dead buttons — the honest v1 state.

import { useState } from "react";
import { api, type WfRun } from "../../../api";
import type { IconName } from "../../../components/Icon";
import { Icon } from "../../../components/Icon";
import { useAppStore } from "../../../store";
import { pausedLabel } from "../status";

interface Action {
  label: string;
  icon: IconName;
  run: (runId: string) => Promise<void>;
  primary?: boolean;
}

interface BannerSpec {
  tone: "amber" | "danger";
  title: string;
  body: string;
  actions: Action[];
  /** A future-slice note shown in place of an action that isn't wired yet. */
  pending?: string;
}

const APPROVE: Action = { label: "Approve", icon: "check", run: api.wfApprove, primary: true };
const RETRY: Action = { label: "Retry", icon: "refresh", run: api.wfRetry, primary: true };
const RESUME: Action = { label: "Resume", icon: "play", run: api.wfResume, primary: true };
// Conflict resolution (§12.3): let an agent resolve the pinned conflict snapshot,
// or continue after the user has resolved it in the run repo's integration
// worktree and committed.
const RESOLVE_AGENT: Action = {
  label: "Resolve with agent",
  icon: "play",
  run: (id) => api.wfResolveConflict(id, "agent"),
  primary: true,
};
const RESOLVE_HUMAN: Action = {
  label: "I resolved it — continue",
  icon: "check",
  run: (id) => api.wfResolveConflict(id, "human"),
};

function specFor(run: WfRun, detail?: string): BannerSpec | null {
  if (run.status === "failed") {
    return {
      tone: "danger",
      title: "Run failed",
      body: run.error || "The run stopped with an unrecoverable error.",
      actions: [],
    };
  }
  if (run.status !== "paused" || !run.paused_reason) return null;

  const title = `Paused — ${pausedLabel(run.paused_reason)}`;
  switch (run.paused_reason) {
    case "approval":
      return {
        tone: "amber",
        title,
        body: detail || "A step is waiting for your approval to hand off to the next.",
        actions: [APPROVE],
      };
    case "blocked_gate":
      return {
        tone: "amber",
        title,
        body: detail || "A step finished but its gate isn't satisfied yet.",
        actions: [RETRY],
      };
    case "stalled":
      return {
        tone: "amber",
        title,
        body: detail || "A step stopped making progress and didn't recover after a nudge.",
        actions: [RETRY, RESUME],
      };
    case "budget_exceeded":
      return {
        tone: "amber",
        title,
        body: detail || "The run reached a configured budget. Resume to keep going.",
        actions: [RESUME],
      };
    case "question":
      return {
        tone: "amber",
        title,
        body: detail || "A step asked a question and is waiting for an answer.",
        actions: [],
        pending: "Answering from the monitor arrives with the comms slice.",
      };
    case "conflict":
      return {
        tone: "amber",
        title,
        body:
          detail ||
          "Merging parallel work hit a conflict. Let an agent resolve it, or resolve it " +
            "yourself in the run's integration worktree and continue.",
        actions: [RESOLVE_AGENT, RESOLVE_HUMAN],
      };
  }
}

export function PausedBanner({ run, detail }: { run: WfRun; detail?: string }) {
  const [busy, setBusy] = useState(false);
  const setLastError = useAppStore((s) => s.setLastError);
  const spec = specFor(run, detail);
  if (!spec) return null;

  const onAct = async (action: Action) => {
    if (busy) return;
    setBusy(true);
    try {
      await action.run(run.id);
      // The row updates via the `wf:run` subscription; nothing to do on success.
    } catch (e) {
      // Surface the failure inline rather than leaving a silent no-op.
      setLastError(`Action failed: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={`wf-banner ${spec.tone}`}>
      <span className="wf-banner-icon">
        <Icon name={spec.tone === "danger" ? "close" : "pause"} size={14} />
      </span>
      <div className="wf-banner-text">
        <div className="wf-banner-title">{spec.title}</div>
        <div className="wf-banner-body">{spec.body}</div>
      </div>
      <div className="wf-banner-actions">
        {spec.pending && <span className="wf-banner-pending">{spec.pending}</span>}
        {spec.actions.map((a) => (
          <button
            key={a.label}
            type="button"
            className={`btn-t ${a.primary ? "primary" : "outline"}`}
            disabled={busy}
            onClick={() => void onAct(a)}
          >
            <Icon name={a.icon} size={13} /> {a.label}
          </button>
        ))}
      </div>
    </div>
  );
}
