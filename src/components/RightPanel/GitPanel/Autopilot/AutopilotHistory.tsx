import { useState } from "react";
import { Icon } from "@/components/Icon";
import { gaveUpLabel, rungNoun } from "@/helpers/autopilotCopy";
import { useAppStore } from "@/store";
import type { AutopilotLogEntry } from "@/store/autopilotLog";
import { checkoutKey } from "@/store/git";

// ── What the agent did on this PR by itself ───────────────────────────────────
// A section of the scrollable body, under the PR card or the file list — with
// the thing it describes, not next to the action button. The first question a
// user has about a loop that ran while they were away is "what did it already
// do, and what did that cost?", so this is a receipt: collapsed by default,
// absent entirely until autopilot has done something worth reading, and only
// the rows that mean trouble take colour.

/** One row's phrasing, past tense — this already happened. A give-up says why,
 *  as a fact about the PR (`gaveUpLabel`); it is the row a returning user is
 *  looking for, and the only place the reason is ever shown. */
export function eventLabel(entry: AutopilotLogEntry): string {
  switch (entry.outcome) {
    case "dispatch":
      return "Handed to the agent";
    case "settle":
      return "Worked";
    case "retry":
      return "Didn't work — trying again";
    case "give-up":
      return entry.reason ? gaveUpLabel(entry.reason, entry.rung) : "Gave up";
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

  // Nothing has happened yet: say nothing. An empty section on every checkout
  // would be noise in a body that should be about the changes.
  if (!log?.length) return null;

  return (
    <section className="ap-log">
      <button
        type="button"
        className="ap-log-h text-xs"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        title="What the agent has done on this PR by itself"
      >
        <Icon name={open ? "chevD" : "chevR"} />
        <span>Autopilot</span>
        <span className="ap-log-sum">
          {log.length === 1 ? "1 action" : `${log.length} actions`}
        </span>
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
    </section>
  );
}
