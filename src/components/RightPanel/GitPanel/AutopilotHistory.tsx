import { useState } from "react";
import { Icon } from "@/components/Icon";
import type { DelegationKind } from "@/delegation";
import { useAppStore } from "@/store";
import type { AutopilotLogEntry } from "@/store/autopilotLog";
import { checkoutKey } from "@/store/git";
import { stuckLabel } from "./AutopilotChip";

// ── What autopilot did on this checkout ───────────────────────────────────────
// Directly under the chip, because the chip answers "what is it doing now?" and
// the very next question a user has — especially about a loop that ran while they
// were away — is "what did it already do, and what did that cost?". Collapsed by
// default: it is a receipt, not a dashboard, and absent entirely until autopilot
// has done something worth reading.

/** The rung as a thing on the PR, not as an action name — a log row reads as
 *  "what it was working on". Partial because an escalation can name a rung
 *  autopilot doesn't drive (`needs-human` on a commit), and the raw kind is a
 *  perfectly honest fallback for those. */
const RUNG_NOUN: Partial<Record<DelegationKind, string>> = {
  "fix-checks": "failing checks",
  resolve: "conflicts",
  "update-branch": "branch update",
  "resolve-comments": "review comments",
};

const rungNoun = (kind: DelegationKind): string => RUNG_NOUN[kind] ?? kind;

/** One row's phrasing, past tense — this already happened. Escalations reuse the
 *  chip's `stuckLabel`, so the reason a user reads in the log is worded exactly
 *  like the one that stopped the chip. */
export function eventLabel(entry: AutopilotLogEntry): string {
  switch (entry.outcome) {
    case "dispatch":
      return "Handed to the agent";
    case "settle":
      return "Worked";
    case "retry":
      return "Didn't work — trying again";
    case "escalate":
      return entry.reason ? stuckLabel(entry.reason) : "Autopilot stopped";
  }
}

/** Clock time of the event. Formats the timestamp the driver recorded — the
 *  entry's own `at` — rather than reading a clock here, so a row can't drift or
 *  disagree with the moment it describes. */
const clock = (at: number) =>
  new Date(at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });

export function AutopilotHistory({ agentId, subdir }: { agentId: string; subdir?: string }) {
  const key = checkoutKey(agentId, subdir);
  const log = useAppStore((s) => s.autopilotLog[key]);
  const [open, setOpen] = useState(false);

  // Nothing has happened yet: say nothing. An empty "history" affordance on every
  // checkout would be noise in a footer that is already dense.
  if (!log?.length) return null;

  return (
    <div className="ap-log">
      <button
        type="button"
        className="ap-log-toggle text-xs"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        title="What autopilot has done on this checkout"
      >
        <Icon name={open ? "chevD" : "chevR"} />
        <Icon name="history" />
        <span>Autopilot history</span>
        <span className="ap-log-count">{log.length}</span>
      </button>

      {open && (
        // Newest first, as stored — the last thing it did is the thing being
        // asked about.
        <ol className="ap-log-list text-xs">
          {log.map((entry) => (
            <li
              // The driver applies at most one effect per checkout per tick, so
              // the stamp plus what happened identifies the row — and unlike the
              // array index it survives the oldest entry being pruned.
              key={`${entry.at}-${entry.outcome}-${entry.rung}`}
              className={`ap-log-row o-${entry.outcome}`}
            >
              <span className="ap-log-time">{clock(entry.at)}</span>
              <span className="ap-log-what">{eventLabel(entry)}</span>
              {entry.rung && <span className="ap-log-rung">{rungNoun(entry.rung)}</span>}
              {/* The attempt number is the budget being spent — the number that
               *  explains why it eventually gave up. */}
              {entry.attempt != null && entry.attempt > 1 && (
                <span className="ap-attempt">#{entry.attempt}</span>
              )}
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}
