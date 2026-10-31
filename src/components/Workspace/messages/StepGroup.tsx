// One run of steps — the agent working between two things it said — as one
// line. Live, the line says what it's doing now ("Reading index.tsx"), shimmer
// and all; settled, what it did ("Changed 9 files +53 −20 · ran 2 commands").
// Either way it opens to the steps, and each step to its detail; a run of one
// opens straight to that detail.

import { memo, useMemo, useState } from "react";
import { Icon, type IconName } from "@/components/Icon";
import { Loader } from "@/components/ui/Loader";
import { taskForToolUse, useAppStore } from "@/store";
import { firstLine } from "@/util/format";
import {
  DeepOpen,
  DisclosureBody,
  Ticker,
  useDisclosure,
  useFocusInside,
  useRevealed,
  useSteady,
} from "./Disclosure";
import { MessageItem } from "./MessageItem";
import type { ViewItem } from "./pair";
import { getPresenter } from "./presenters";
import { DiffCount } from "./presenters/util";
import type { StepRun } from "./rows";
import { liveActivity, runIcon, type SummaryPart, summarizeSteps, summaryParts } from "./steps";
import { ToolRow } from "./ToolRow";

/** The summary line: phrases joined by a quiet dot, the first capitalized
 *  unless a `lead` phrase ("Worked for 4m 12s") goes before them. */
export function SummaryLine({
  lead,
  parts,
  note,
}: {
  lead?: string;
  parts: SummaryPart[];
  /** Muted trailing text, e.g. a lone thought's first line. */
  note?: string;
}) {
  return (
    <span className="steps-summary">
      {lead && <span className="steps-lead">{lead}</span>}
      {parts.map((p, i) => {
        const text = i === 0 && !lead ? p.text.charAt(0).toUpperCase() + p.text.slice(1) : p.text;
        return (
          <span key={p.key} className={p.tone ? `steps-part ${p.tone}` : "steps-part"}>
            {(i > 0 || lead) && <span className="steps-sep">·</span>}
            {text}
            {p.code && <code>{p.code}</code>}
            {p.diff && <DiffCount additions={p.diff.additions} deletions={p.diff.deletions} />}
          </span>
        );
      })}
      {note && <span className="steps-note">{note}</span>}
    </span>
  );
}

/** The leading glyph: what the run is about at rest, a chevron on hover and
 *  while open. A live run's icon follows what it's doing now. */
export function StepGlyph({ icon }: { icon: IconName }) {
  return (
    <span className="steps-glyph" aria-hidden>
      <Ticker k={icon} className="steps-icon">
        <Icon name={icon} size={12} />
      </Ticker>
      <Icon name="chevR" size={12} className="steps-chev" />
    </span>
  );
}

/** A thought, as a row among the steps: its first line, opening to the rest. */
function ThoughtRow({ text }: { text: string }) {
  const lower = useMemo(() => text.toLowerCase(), [text]);
  return (
    <ToolRow
      name="Thought"
      icon="sparkle"
      summary={<span className="t-thought">{firstLine(text, 160)}</span>}
      expanded={<div className="tb-thought">{text}</div>}
      revealText={lower}
    />
  );
}

/** One step inside an open run. */
function StepItem({
  item,
  agentId,
  provider,
  busy,
}: {
  item: ViewItem;
  agentId?: string;
  provider?: string;
  busy: boolean;
}) {
  if (item.kind === "notice" && item.subtype === "reasoning")
    return <ThoughtRow text={item.text} />;
  return <MessageItem item={item} agentId={agentId} provider={provider} busy={busy} />;
}

/** A run of one opens to that step's own detail; there's no list to show. */
function SoleDetail({
  item,
  agentId,
  provider,
  busy,
}: {
  item: ViewItem;
  agentId?: string;
  provider?: string;
  busy: boolean;
}) {
  if (item.kind === "tool_pair") {
    return (
      <div className="tool-body is-sole">
        {getPresenter(item.call.name).expanded(item.call, item.result)}
      </div>
    );
  }
  if (item.kind === "notice" && item.subtype === "reasoning") {
    return (
      <div className="tool-body is-sole">
        <div className="tb-thought">{item.text}</div>
      </div>
    );
  }
  return <MessageItem item={item} agentId={agentId} provider={provider} busy={busy} />;
}

export const StepGroup = memo(function StepGroup({
  run,
  agentId,
  provider,
  busy,
  tail,
}: {
  run: StepRun;
  agentId?: string;
  provider?: string;
  /** The run sits in the turn that's live right now. */
  busy: boolean;
  /** Nothing comes after the run yet: while the turn is live, the agent is
   *  still on it. */
  tail: boolean;
}) {
  const { items } = run;
  const { open, deep, instant, reveal, toggle } = useDisclosure();
  const revealed = useRevealed(items);
  useFocusInside(agentId, items, reveal);
  const shown = open || revealed;

  const summary = useMemo(() => summarizeSteps(items), [items]);
  const parts = useMemo(() => summaryParts(summary), [summary]);
  const pending = busy && items.some((it) => it.kind === "tool_pair" && !it.result);
  const live = busy && (tail || pending);
  const raw = liveActivity(items);
  const activity = useSteady(raw, raw.label, raw.idle);
  const label = activity.label;

  // Background work this run launched that outlives it (a dev server, a test
  // watcher): the header keeps saying so after the run has settled.
  const tasks = useAppStore((s) => (agentId ? s.backgroundTasks[agentId] : undefined));
  const running = useMemo(
    () =>
      items.filter(
        (it) => it.kind === "tool_pair" && taskForToolUse(tasks, it.call.id)?.status === "running",
      ).length,
    [items, tasks],
  );

  // A run that appears live rises in; one already there on load doesn't.
  const [fresh] = useState(live);
  const sole = items.length === 1 ? items[0] : null;
  const thoughtOnly = parts.length === 1 && parts[0].key === "thought";
  const firstThought = items.find((it) => it.kind === "notice" && it.subtype === "reasoning");
  const icon = live ? activity.icon : runIcon(items, summary);

  const cls = ["steps", live && "is-live", shown && "is-open", fresh && "is-fresh"]
    .filter(Boolean)
    .join(" ");
  return (
    <div className={cls} data-steps={run.key}>
      <button type="button" className="steps-head" aria-expanded={shown} onClick={toggle}>
        <StepGlyph icon={icon} />
        <Ticker k={live ? `live:${label}` : "settled"} className="steps-text">
          {live ? (
            <span className="steps-live">{label}</span>
          ) : (
            <SummaryLine
              parts={parts}
              note={
                thoughtOnly && firstThought?.kind === "notice"
                  ? firstLine(firstThought.text, 140)
                  : undefined
              }
            />
          )}
        </Ticker>
        {running > 0 && (
          <span className="steps-aside">
            <Loader variant="muted" size="sm" />
            {running} running
          </span>
        )}
        {live && summary.calls > 1 && (
          <span className="steps-aside steps-count">{summary.calls} steps</span>
        )}
      </button>
      <DisclosureBody open={shown} instant={instant || (revealed && !open)} className="steps-body">
        <DeepOpen.Provider value={deep}>
          {sole ? (
            <SoleDetail item={sole} agentId={agentId} provider={provider} busy={busy} />
          ) : (
            <div className="steps-list">
              {items.map((item, i) => (
                <div
                  key={stepKey(item, run.start + i)}
                  className="steps-item"
                  style={{ "--i": Math.min(i, 12) } as React.CSSProperties}
                >
                  <StepItem item={item} agentId={agentId} provider={provider} busy={busy} />
                </div>
              ))}
            </div>
          )}
        </DeepOpen.Provider>
      </DisclosureBody>
    </div>
  );
});

function stepKey(item: ViewItem, index: number): string {
  if (item.kind === "tool_pair" && item.call.id) return `tp:${item.call.id}`;
  return `i:${index}`;
}
