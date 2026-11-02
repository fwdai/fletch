// One run of steps — the agent working between two things it said — as one
// line. Live, the line says what it's doing now ("Reading index.tsx"), shimmer
// and all; between two calls, what it has done so far, trailed by dots; settled,
// what it did ("Changed 9 files +53 −20 · ran 2 commands"). Either way it opens
// to the steps, and each step to its detail; a run of one opens straight to
// that detail.
//
// Shared by the desktop chat and the phone: the steps themselves (tool rows,
// thoughts) render here, through the presenters; anything else that lands in a
// run (a hook's output) goes back to the app's own `renderItem`.

import { memo, type ReactNode, useMemo, useState } from "react";
import { Icon, type IconName } from "@/components/Icon";
import { Loader } from "@/components/ui/Loader";
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
import { useTranscriptHost } from "./host";
import { itemIdentity, type ViewItem } from "./pair";
import { getPresenter, type ToolCall, type ToolResult } from "./presenters";
import { Caption } from "./presenters/blocks";
import { DiffCount } from "./presenters/util";
import type { StepRun } from "./rows";
import {
  liveActivity,
  runIcon,
  type SummaryPart,
  searchText,
  stepStatus,
  summarizeSteps,
  summaryParts,
} from "./steps";
import { ToolResultItem } from "./ToolResultItem";
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
    <span className="steprun-summary">
      {lead && <span className="steprun-lead">{lead}</span>}
      {parts.map((p, i) => {
        const text = i === 0 && !lead ? p.text.charAt(0).toUpperCase() + p.text.slice(1) : p.text;
        return (
          <span key={p.key} className={p.tone ? `steprun-part ${p.tone}` : "steprun-part"}>
            {(i > 0 || lead) && <span className="steprun-sep">·</span>}
            {text}
            {p.code && <code>{p.code}</code>}
            {p.diff && <DiffCount additions={p.diff.additions} deletions={p.diff.deletions} />}
          </span>
        );
      })}
      {note && <span className="steprun-note">{note}</span>}
    </span>
  );
}

/** The leading glyph: what the run is about at rest, a chevron on hover and
 *  while open. A live run's icon follows what it's doing now. */
export function StepGlyph({ icon }: { icon: IconName }) {
  return (
    <span className="steprun-glyph" aria-hidden>
      <Ticker k={icon} className="steprun-icon">
        <Icon name={icon} size={12} />
      </Ticker>
      <Icon name="chevR" size={12} className="steprun-chev" />
    </span>
  );
}

/** One tool call as a row: its presenter's icon, label, summary and detail.
 *  In flight while the agent is busy and the call has no result yet (a long
 *  Bash), or while the background task it launched is still running — its
 *  launch result lands at once, so `busy && !result` alone would settle the
 *  row too early. Used wherever a tool call is drawn: inside a run here, and
 *  by each app's own message renderer for calls that aren't steps. */
export function ToolStep({
  call,
  result,
  agentId,
  busy,
}: {
  call: ToolCall;
  result: ToolResult | null;
  agentId?: string;
  busy: boolean;
}) {
  const presenter = getPresenter(call.name);
  const status = stepStatus(result, useTranscriptHost().taskFor(call.id), busy);
  const pair = useMemo(() => ({ kind: "tool_pair" as const, call, result }), [call, result]);
  return (
    <ToolRow
      name={presenter.title ?? call.name}
      icon={presenter.icon}
      isError={status.failed}
      running={status.running}
      toolUseId={call.id}
      agentId={agentId}
      summary={
        <>
          {presenter.summary(call, result)}
          {status.note && (
            <span style={{ color: "var(--danger)", marginLeft: 8 }}>· {status.note}</span>
          )}
        </>
      }
      expanded={presenter.expanded(call, result)}
      revealText={searchText(pair)}
    />
  );
}

/** A thought in full. Every reasoning notice renders through here: as a row's
 *  body in a run of several steps, or as the whole body of a run of one. */
function ThoughtBody({ text }: { text: string }) {
  return <div className="tb-thought">{text}</div>;
}

/** A thought, as a row among the steps: its first line, opening to the rest. */
function ThoughtRow({ text }: { text: string }) {
  const lower = useMemo(() => text.toLowerCase(), [text]);
  return (
    <ToolRow
      name="Thought"
      icon="sparkle"
      summary={<span className="t-thought">{firstLine(text.trimStart(), 160)}</span>}
      expanded={<ThoughtBody text={text} />}
      revealText={lower}
    />
  );
}

type RenderItem = (index: number, busy: boolean) => ReactNode;

/** One step inside an open run. */
function StepItem({
  item,
  index,
  agentId,
  busy,
  renderItem,
}: {
  item: ViewItem;
  index: number;
  agentId?: string;
  busy: boolean;
  renderItem: RenderItem;
}) {
  switch (item.kind) {
    case "notice":
      if (item.subtype === "reasoning") return <ThoughtRow text={item.text} />;
      return <>{renderItem(index, busy)}</>;
    case "tool_pair":
      return <ToolStep call={item.call} result={item.result} agentId={agentId} busy={busy} />;
    case "tool_call":
      return <ToolStep call={item} result={null} agentId={agentId} busy={busy} />;
    case "tool_result":
      return <ToolResultItem item={item} />;
    default:
      return <>{renderItem(index, busy)}</>;
  }
}

/** A run of one opens to that step's own detail; there's no list to show.
 *  A failed background task has no row to say so here, so its status leads
 *  the detail. */
function SoleDetail(props: {
  item: ViewItem;
  index: number;
  agentId?: string;
  busy: boolean;
  renderItem: RenderItem;
}) {
  const { item, busy } = props;
  const { taskFor } = useTranscriptHost();
  if (item.kind === "tool_pair") {
    const status = stepStatus(item.result, taskFor(item.call.id), busy);
    return (
      <div className="tool-body is-sole">
        {status.note && <Caption tone="danger">{status.note}</Caption>}
        {getPresenter(item.call.name).expanded(item.call, item.result)}
      </div>
    );
  }
  if (item.kind === "notice" && item.subtype === "reasoning") {
    return (
      <div className="tool-body is-sole">
        <ThoughtBody text={item.text} />
      </div>
    );
  }
  return <StepItem {...props} />;
}

export const StepGroup = memo(function StepGroup({
  run,
  agentId,
  provider: _provider,
  busy,
  tail,
  renderItem,
}: {
  run: StepRun;
  agentId?: string;
  provider?: string;
  /** The run sits in the turn that's live right now. */
  busy: boolean;
  /** Nothing comes after the run yet: while the turn is live, the agent is
   *  still on it. */
  tail: boolean;
  /** The app's rendering of an item in the run that isn't a tool call or a
   *  thought (see RowEnv). */
  renderItem: RenderItem;
}) {
  const { items } = run;
  const { open, deep, instant, reveal, toggle } = useDisclosure();
  const revealed = useRevealed(items);
  useFocusInside(agentId, items, reveal);
  const shown = open || revealed;

  // The summary reads the background tasks too: work this run launched that
  // outlives it (a dev server, a watcher) keeps the header saying "running",
  // and a command that failed after its launch result landed counts as failed.
  const { taskFor } = useTranscriptHost();
  const summary = useMemo(() => summarizeSteps(items, taskFor), [items, taskFor]);
  const parts = useMemo(() => summaryParts(summary), [summary]);
  const pending = busy && items.some((it) => it.kind === "tool_pair" && !it.result);
  const live = busy && (tail || pending);
  const raw = liveActivity(items);
  const activity = useSteady(raw, raw.label, raw.idle);
  const label = activity.label;
  const running = summary.running;
  // Between two calls (the last came back, the next hasn't begun) nothing is
  // in flight to name, and a generic "Working" would only repeat the strip
  // above the composer. So the line says what the run has done so far, with
  // dots for the rest to come, and settles into that same line, minus the
  // dots, when the run ends.
  const between = live && activity.idle;
  const doing = live && !between;

  // A run that appears live rises in; one already there on load doesn't.
  const [fresh] = useState(live);
  const sole = items.length === 1 ? items[0] : null;
  // A run that only thought says what it thought, in brief.
  const note =
    parts.length === 1 && parts[0].key === "thought" && summary.firstThought
      ? firstLine(summary.firstThought.trimStart(), 140)
      : undefined;
  const icon = doing ? activity.icon : runIcon(items, summary);

  const cls = ["steprun", live && "is-live", shown && "is-open", fresh && "is-fresh"]
    .filter(Boolean)
    .join(" ");
  return (
    <div className={cls} data-steprun={run.key}>
      <button type="button" className="steprun-head" aria-expanded={shown} onClick={toggle}>
        <StepGlyph icon={icon} />
        <Ticker k={doing ? `live:${label}` : "settled"} className="steprun-text">
          {doing ? (
            <span className="steprun-live">{label}</span>
          ) : (
            <SummaryLine parts={parts} note={note} />
          )}
        </Ticker>
        {between && (
          <span className="steprun-aside steprun-more">
            <Loader variant="muted" size="sm" aria-hidden />
          </span>
        )}
        {running > 0 && (
          <span className="steprun-aside">
            <Loader variant="muted" size="sm" />
            {running} running
          </span>
        )}
        {doing && summary.calls > 1 && (
          <span className="steprun-aside steps-count">{summary.calls} steps</span>
        )}
      </button>
      <DisclosureBody
        open={shown}
        instant={instant || (revealed && !open)}
        className="steprun-body"
      >
        <DeepOpen.Provider value={deep}>
          {sole ? (
            <SoleDetail
              item={sole}
              index={run.start}
              agentId={agentId}
              busy={busy}
              renderItem={renderItem}
            />
          ) : (
            <div className="steprun-list">
              {items.map((item, i) => (
                <div
                  key={stepKey(item, run.start + i)}
                  className="steprun-item"
                  style={{ "--i": Math.min(i, 12) } as React.CSSProperties}
                >
                  <StepItem
                    item={item}
                    index={run.start + i}
                    agentId={agentId}
                    busy={busy}
                    renderItem={renderItem}
                  />
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
  return itemIdentity(item) ?? `i:${index}`;
}
