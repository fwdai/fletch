import { open as openExternal } from "@tauri-apps/plugin-shell";
import { useState } from "react";
import type {
  RoadmapItem,
  RoadmapItemEvent,
  RoadmapProposal,
  RoadmapProposalPatch,
  WfRun,
} from "@/api";
import { Icon, type IconName } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { formatAge } from "@/util/format";
import { pausedLabel } from "@/workflows/run/status";
import { EVENT_LABEL, eventDetailUrl, eventLine } from "../itemHistory";
import { buildProposalDiff, isEmptyDiff } from "../proposalDiff";
import type { BoardItem, ItemSource, ItemStatus } from "../types";
import { DecisionBar } from "./DecisionBar";
import type { CardDnd } from "./useBoardDnd";

/** Where the item came from, as a one-glyph tag. */
const SOURCE: Record<ItemSource, { icon: IconName; tip: string }> = {
  user: { icon: "user", tip: "Added by hand" },
  pm: { icon: "sparkle", tip: "Written here with the PM agent" },
  linear: { icon: "layers", tip: "From Linear" },
  github: { icon: "github", tip: "From GitHub" },
};

/** The one-word state chip on the header line, for the statuses that mean
 *  something is (or should be) happening. `open`/`proposed` get none — an item
 *  nobody has queued is the board's resting state and needs no label. */
const STATE: Partial<Record<ItemStatus, { label: string; cls: string; tip: string }>> = {
  queued: {
    label: "Queued",
    cls: "q",
    tip: "Waiting for a slot — the queue runs one item per project at a time",
  },
  in_review: {
    label: "In review",
    cls: "r",
    // The watch is host-side (src-tauri/src/roadmap/merge_sweep.rs), so this is
    // a promise the app keeps with the window closed — worth saying, because it
    // is the difference between "check back here" and "go merge it". "Mark
    // done" on the card is the manual fallback for when the watch can't see
    // the merge (revoked token, deleted PR).
    tip: "Its run opened a pull request — merge it and this ships on its own",
  },
};

interface Props {
  item: BoardItem;
  /** The project's primary repo — where "Send to an agent" opens the draft. */
  repoPath: string;
  /** A proposed row: on the board, but not on the roadmap until it's accepted. */
  ghost?: boolean;
  open: boolean;
  onToggle: () => void;
  /** Accept the proposal (`proposed → open`). Ghosts only, and not read-only. */
  onAccept?: () => void;
  /** Discard the proposal — the row is deleted. Ghosts only. */
  onDiscard?: () => void;
  /** The PM's pending ask against this row — a change or a discard the user
   *  hasn't ruled on. Drawn as an always-visible bar (the ghost bar's grammar)
   *  plus, for a change, a per-field diff in the expanded body. */
  proposal?: RoadmapProposal;
  /** Apply the pending ask. Absent on a read-only board. */
  onAcceptProposal?: () => void;
  /** Decline it — the item stays as it is, the refusal lands in history. */
  onRejectProposal?: () => void;
  /** Hand the item to the queue (`open → queued`). Absent for a ghost and on a
   *  read-only board. */
  onQueue?: () => void;
  /** Take it back off the queue before it's dispatched (`queued → open`). */
  onUnqueue?: () => void;
  /** Take the item back off the agent it was handed to — clears `agent_id` and
   *  returns the row to the queue's world. Only on a handed-off row that hasn't
   *  been dispatched since (`proposed | open`), and not read-only. */
  onReclaim?: () => void;
  /** Ship it by hand (`in_review → done`) when the sweep can't see the merge —
   *  a revoked token, a deleted PR, a repo that left the project. In-review
   *  items only, and not read-only. */
  onMarkDone?: () => void;
  /** Open the run this item is being built by. Only on an item with a run. */
  onOpenRun?: () => void;
  /** The live row of the run this item is tied to (`run_id`), when the workflow
   *  engine still has one. Read for the pearl's label and, when the run is
   *  paused, for the reason chip — an `active` card that has stopped moving must
   *  say why without a trip to the monitor. */
  run?: WfRun;
  /** The workflow this item would run under ("Project default" resolved), or
   *  null when nothing would run it — the queue would stall on it. */
  workflowName?: string | null;
  /** Why this item isn't moving, straight from the queue: the drainer's reason
   *  a queued row is stuck, or the merge sweep's "PR #N was closed without
   *  merging" for one that came back off review. */
  note?: string;
  /** The item's durable history, newest first — fetched lazily on first expand
   *  (see `useRoadmap.loadEvents`), so it can be absent for a beat. */
  events?: RoadmapItemEvent[];
  /** Ring the row: it was just jumped to, or a pending proposal moves it. */
  focused?: boolean;
  /** Drag-to-reorder wiring, from `useBoardDnd`. Absent on a read-only board and
   *  for rows nothing can reorder. The card only draws it. */
  dnd?: CardDnd;
  /** Transient highlight for a row that just landed or just moved. */
  landed?: boolean;
  /** Open this item's form. Absent for a ghost (there is no row to edit yet)
   *  and on a read-only board. */
  onEdit?: () => void;
  /** Lets the board scroll a focused row into view. */
  cardRef?: (el: HTMLDivElement | null) => void;
}

/** The item as a starting prompt for an agent: what to build, why, and what
 *  "done" means. Prefills the new draft's composer so the user reviews and
 *  launches rather than retyping the row. */
function briefFor(item: BoardItem): string {
  const lines = [`${item.code}: ${item.title}`, "", item.why];
  if (item.accept?.length) lines.push("", "Done when:", ...item.accept.map((a) => `- ${a}`));
  return lines.join("\n").trim();
}

/** One roadmap row: a click-to-expand header line (code, title, source)
 *  over the rationale, acceptance criteria and dependencies. */
export function ItemCard({
  item,
  repoPath,
  ghost,
  open,
  onToggle,
  focused,
  landed,
  onEdit,
  onAccept,
  onDiscard,
  proposal,
  onAcceptProposal,
  onRejectProposal,
  onQueue,
  onUnqueue,
  onReclaim,
  onMarkDone,
  onOpenRun,
  run,
  workflowName,
  note,
  events,
  dnd,
  cardRef,
}: Props) {
  const createDraft = useAppStore((s) => s.createDraft);
  const closeProjectScreen = useAppStore((s) => s.closeProjectScreen);
  const selectAgent = useAppStore((s) => s.selectAgent);
  const agentId = item.item.agent_id;
  /** The agent named on the row, if the workspace still exists. A hand-off is
   *  recorded permanently but the agent it names is disposable, so a stale link
   *  resolves to nothing — the row says it is spoken for without offering a
   *  dangling id to click. */
  const agentName = useAppStore((s) =>
    agentId ? (s.workspace?.agents.find((a) => a.id === agentId)?.name ?? null) : null,
  );
  /** The manual hand-off ("Send to an agent"): an agent is on this item and no
   *  run owns it, so the queue isn't driving — the user is.
   *
   *  Keyed on the *stamp*, not on the name resolving: the stamp is what hides
   *  Queue and what makes the drainer skip the row, so an item whose agent has
   *  since been deleted must still show that — otherwise it sits on the board
   *  with no builder, no queue affordance, and no explanation. That row is
   *  exactly the one that needs "Take it back". */
  const handedOff = !!agentId && !item.item.run_id;
  /** In review with a PR the merge sweep can't poll: it has the URL but no
   *  number, which is exactly the state `merge_sweep::pollable` skips. */
  const unpollable =
    item.status === "in_review" && !!item.item.pr_url && item.item.pr_number == null;
  const paused = run?.status === "paused" ? run.paused_reason : null;
  const source = SOURCE[item.source];
  const state = STATE[item.status];
  const cls = [
    "rm-item",
    ghost ? "ghost" : "",
    proposal ? "prop" : "",
    open ? "open" : "",
    landed ? "landed" : "",
    focused ? "focus" : "",
    item.status === "queued" ? "queued" : "",
    // The whole row is the drag handle — a card is a single object, and a
    // dedicated grip would be a second affordance for one gesture.
    dnd?.draggable ? "drag" : "",
    dnd?.dragging ? "dragging" : "",
    dnd?.edge ? `drop-${dnd.edge}` : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div
      ref={cardRef}
      className={cls}
      draggable={dnd?.draggable}
      onDragStart={dnd?.onDragStart}
      onDragEnd={dnd?.onDragEnd}
      onDragOver={dnd?.onDragOver}
      onDragLeave={dnd?.onDragLeave}
      onDrop={dnd?.onDrop}
    >
      <button
        type="button"
        className="rm-item-h flex-center"
        onClick={onToggle}
        aria-expanded={open}
      >
        {/* A proposed row already owns its code (the PM quotes it in the chat
            the moment it proposes), and accepting one never renumbers it. */}
        <span className="rm-code mono text-xs">{item.code}</span>
        <span className="rm-title text-sm truncate">{item.title}</span>
        {/* A dispatched item shows the pearl whether or not anything has been
            stamped on it yet: the queue flips it to `active` at the moment it
            claims the row, a beat before the run exists. What the pearl *names*
            is the most specific thing that resolves — the run doing the work,
            else the agent on the row — and only falls back to the bare word when
            neither is loaded yet. */}
        {item.status === "active" && (
          <span className="rm-live iflex-center mono text-xs">
            <span className="rm-pearl" />
            <span className="truncate">{run?.name ?? agentName ?? "running"}</span>
          </span>
        )}
        {/* A paused run is the board's most important state and the one it used
            to hide: the pearl keeps pulsing while nothing happens. Say why here,
            on the row, so the trip to the monitor is a choice. */}
        {paused && (
          <span
            className="rm-paused iflex-center text-xs"
            title="Its run is waiting on something — open the run to deal with it"
          >
            <Icon name="hand" size={11} />
            Paused — {pausedLabel(paused)}
          </span>
        )}
        {state && (
          <span className={`rm-state iflex-center text-xs st-${state.cls}`} title={state.tip}>
            {state.label}
          </span>
        )}
        <span className={`rm-src iflex-center src-${item.source}`} title={source.tip}>
          <Icon name={source.icon} size={11} />
        </span>
        <Icon name="chevD" size={11} className="rm-chev" />
      </button>

      {/* The two buttons that decide a proposal's fate. Outside the header
          button (no nesting) and outside the collapsible body, so ruling on a
          ghost never costs an expand — reading it first is what the expand is
          for. */}
      {ghost && (onAccept || onDiscard) && (
        <DecisionBar
          label="Proposed — not on the roadmap yet"
          declineLabel="Discard"
          onAccept={onAccept}
          onDecline={onDiscard}
        />
      )}

      {/* The PM's pending ask against this row — the same bar for a delta
          instead of a new ticket: always visible, ruled on without an expand.
          The expanded body carries the per-field diff. Never rendered on a
          ghost: two stacked bars whose Accepts mean different things ("put it
          on the board" vs "apply the patch") is a coin-flip for the user —
          rule on the ghost first, the ask stays pending. */}
      {proposal && !ghost && (onAcceptProposal || onRejectProposal) && (
        <DecisionBar
          variant="prop"
          label={proposal.kind === "discard" ? "PM proposes discarding" : "PM proposes changes"}
          note={proposal.note}
          declineLabel="Decline"
          onAccept={onAcceptProposal}
          onDecline={onRejectProposal}
        />
      )}

      {/* The manual hand-off, visible without an expand: the queue doesn't own
          this item, an agent does. Clicking goes there — the agent lives in the
          workspace this full-screen page covers, so selecting it and getting out
          of the way is the whole navigation (same move as "View run") — and
          "Take it back" is the way out of the state entirely. */}
      {handedOff && (
        <div className="rm-handoff-row flex-center">
          {agentName ? (
            <button
              type="button"
              className="rm-handoff flex-center text-xs"
              onClick={() => {
                selectAgent(agentId as string);
                closeProjectScreen();
              }}
            >
              <Icon name="zap" size={11} />
              <span className="rm-handoff-t truncate">Handed to {agentName}</span>
              <Icon name="arrowR" size={11} className="rm-handoff-go" />
            </button>
          ) : (
            // The agent was deleted after the hand-off. Nowhere to go, so this
            // is a statement rather than a link — but the stamp is still what
            // keeps the row out of the queue, so it has to be visible.
            <span className="rm-handoff flex-center text-xs">
              <Icon name="zap" size={11} />
              <span className="rm-handoff-t truncate">
                Handed to an agent that no longer exists
              </span>
            </span>
          )}
          {/* The undo, beside the fact it undoes: a hand-off is the item's
              dispatch, so taking it back is the only way the row returns to the
              queue's world. Its own command rather than a patch, so the trail
              says which agent it came back from (`roadmapReclaimItem`). */}
          {onReclaim && (
            <button
              type="button"
              className="rm-handoff-undo text-xs"
              title={
                agentName
                  ? `Clear ${agentName} off this item and put it back on the board`
                  : "Clear the agent off this item and put it back on the board"
              }
              onClick={onReclaim}
            >
              Take it back
            </button>
          )}
        </div>
      )}

      {/* Why a queued row isn't moving. Outside the collapsible body and
          outside the header button, like the ghostbar: an item that has stalled
          must say so without the user having to go looking for it. */}
      {note && (
        <div className="rm-note flex-center text-xs">
          <Icon name="hand" size={11} />
          <span className="rm-note-t">{note}</span>
        </div>
      )}

      {open && (
        <div className="rm-item-body">
          {/* The pending change, first: the reader came to rule on it, and the
              unchanged body below is the context, not the news. */}
          {proposal?.kind === "update" && proposal.patch && (
            <ProposalDiff item={item.item} patch={proposal.patch} />
          )}
          {item.why && <p className="rm-why text-sm">{item.why}</p>}
          {item.accept && (
            <ul className="rm-accept text-sm">
              {item.accept.map((a) => (
                <li key={a}>{a}</li>
              ))}
            </ul>
          )}
          {/* Two groups rather than one row with a spacer: when the card is too
              narrow to hold both, the actions wrap as a block and stay together
              on the right. A spacer would drop them one at a time, leaving the
              primary button stranded on its own line. */}
          <div className="rm-item-foot">
            <div className="rm-item-meta">
              {item.area && <span className="rm-area mono text-xs">{item.area}</span>}
              {item.deps?.map((d) => (
                <span key={d} className="rm-dep iflex-center mono text-xs">
                  <Icon name="arrowR" size={11} />
                  after {d}
                </span>
              ))}
              {/* What the queue would run this under, so the user knows before
                  they queue rather than after it stalls. */}
              {(onQueue || onUnqueue) && (
                <span
                  className={`rm-wf iflex-center mono text-xs ${workflowName ? "" : "none"}`}
                  title={
                    workflowName
                      ? `Runs under the ${workflowName} workflow`
                      : "No workflow set on this item and no project default — the queue can't run it yet"
                  }
                >
                  <Icon name="combine" size={11} />
                  <span className="truncate">{workflowName ?? "no workflow"}</span>
                </span>
              )}
            </div>
            <div className="rm-item-acts">
              {onEdit && (
                <Button variant="ghost" size="sm" onClick={onEdit}>
                  <Icon name="edit" size={11} /> Edit
                </Button>
              )}
              {/* The manual hand-off, for rows nothing is building yet: the
                queue is autonomous, and sometimes you want to drive. Demoted to
                a ghost button next to "Queue", which is the path most rows
                take. A proposed row isn't work anyone has agreed to do —
                accept it first, then send it. Anything from `queued` on is
                already dispatched (the backend refuses those too), and a row
                that was already handed off keeps its one builder. */}
              {!ghost && item.status === "open" && !agentId && (
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={async () => {
                    // The item id rides the draft: the link can only be recorded
                    // once an agent exists, and a draft's first send is what
                    // spawns one (see `spawnFromDraft`). A draft the user
                    // abandons therefore stamps nothing.
                    const draftId = await createDraft(repoPath, briefFor(item), item.item.id);
                    // The draft lives in the workspace, which this page covers —
                    // but stay put if it couldn't be created, so the user isn't
                    // dropped somewhere else to read the error.
                    if (draftId) closeProjectScreen();
                  }}
                >
                  <Icon name="zap" size={11} /> Send to an agent
                </Button>
              )}
              {/* An item in review is waiting on a PR, so the PR is the thing to
                go to — the run behind it is already finished. */}
              {item.item.pr_url && (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    void openExternal(item.item.pr_url as string).catch(() => {});
                  }}
                >
                  <Icon name="pr" size={11} />
                  {item.item.pr_number ? `PR #${item.item.pr_number}` : "View PR"}
                </Button>
              )}
              {onOpenRun && (
                <Button variant="outline" size="sm" onClick={onOpenRun}>
                  <Icon name="combine" size={11} /> View run
                </Button>
              )}
              {onUnqueue && (
                <Button variant="outline" size="sm" onClick={onUnqueue}>
                  Take off the queue
                </Button>
              )}
              {/* The one in-review state the merge sweep can't act on: a PR it
                  has a link to but no number for, so there is nothing to poll
                  (see merge_sweep.rs `pollable` — a number guessed off the URL
                  would be a wrong verdict written to the board). The card used to
                  say nothing at all, leaving the item to sit in review forever
                  while the "merge it and this ships on its own" promise quietly
                  didn't apply. Sits beside "Mark done", which is the answer. */}
              {unpollable && (
                <span
                  className="rm-unpollable iflex-center text-xs"
                  title="This item's pull request has no number on the row, so the app can't watch it for a merge"
                >
                  <Icon name="hand" size={11} />
                  Can't watch this PR — mark it done when it merges
                </span>
              )}
              {onMarkDone && (
                <Button variant="outline" size="sm" onClick={onMarkDone}>
                  <Icon name="check" size={11} /> Mark done
                </Button>
              )}
              {onQueue && (
                <Button variant="primary" size="sm" onClick={onQueue}>
                  <Icon name="play" size={11} /> Queue
                </Button>
              )}
            </div>
          </div>
          {events && events.length > 0 && <ItemHistory events={events} />}
        </div>
      )}
    </div>
  );
}

/** What the PM's pending change would do to this row, field by field: the old
 *  value struck through, the proposed one highlighted; list fields as merged
 *  lists with additions highlighted and removals struck. Pure pairing lives in
 *  proposalDiff.ts; this only draws it. A patch that would change nothing
 *  (the row caught up already) draws nothing — the bar still says what was
 *  asked. */
function ProposalDiff({ item, patch }: { item: RoadmapItem; patch: RoadmapProposalPatch }) {
  const diff = buildProposalDiff(item, patch);
  if (isEmptyDiff(diff)) return null;
  return (
    <div className="rm-prop-diff text-xs">
      {diff.texts.map((t) => (
        <div key={t.field} className="rm-prop-row">
          <span className="rm-prop-k mono">{t.label}</span>
          <span className="rm-prop-v text-sm">
            {t.from != null && <s className="rm-prop-old">{t.from}</s>}
            {t.to != null ? (
              <span className="rm-prop-new">{t.to}</span>
            ) : (
              // A clear: the strike says what goes; this says nothing replaces it.
              <span className="rm-prop-none">cleared</span>
            )}
          </span>
        </div>
      ))}
      {diff.lists.map((l) => (
        <div key={l.field} className="rm-prop-row">
          <span className="rm-prop-k mono">{l.label}</span>
          <ul className="rm-prop-list text-sm">
            {l.entries.map((e) => (
              <li key={e.text}>
                {e.change === "removed" ? (
                  <s className="rm-prop-old">{e.text}</s>
                ) : e.change === "added" ? (
                  <span className="rm-prop-new">{e.text}</span>
                ) : (
                  e.text
                )}
              </li>
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}

/** The card's history footnote: the latest event inline ("Run failed — its run
 *  failed · 2h ago"), with the full trail behind a disclosure when there is
 *  more than one line to show. Deliberately quiet — this is provenance, not an
 *  action surface. */
function ItemHistory({ events }: { events: RoadmapItemEvent[] }) {
  const [open, setOpen] = useState(false);
  const now = Date.now();
  const latest = events[0];
  /** A line whose detail is nothing but a URL (`pr_opened`) is drawn with the
   *  address as a real link, so the trail's most useful entry is one click
   *  instead of a string to copy out by eye. Plain-text details are untouched.
   *
   *  `linkable` is off wherever the line already sits inside a `<button>` (the
   *  disclosure header): a button inside a button is invalid, and the latest
   *  entry repeats as the first row of the expanded list, where it *is*
   *  clickable. */
  const line = (e: RoadmapItemEvent, linkable = true) => {
    const url = linkable ? eventDetailUrl(e) : null;
    return (
      <>
        <span className="rm-hist-t truncate">
          {url ? (
            <>
              {EVENT_LABEL[e.kind]} —{" "}
              <button
                type="button"
                className="rm-hist-link"
                title={url}
                onClick={() => {
                  void openExternal(url).catch(() => {});
                }}
              >
                {url}
              </button>
            </>
          ) : (
            eventLine(e)
          )}
        </span>
        <span className="rm-hist-age mono">{formatAge(e.created_at, now)}</span>
      </>
    );
  };
  if (events.length === 1) {
    return (
      <div className="rm-hist">
        <div className="rm-hist-h flex-center text-xs">
          <Icon name="history" size={11} />
          {line(latest)}
        </div>
      </div>
    );
  }
  return (
    <div className="rm-hist">
      <button
        type="button"
        className="rm-hist-h flex-center text-xs"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
      >
        <Icon name="history" size={11} />
        {line(latest, false)}
        <Icon name="chevD" size={9} className="rm-hist-chev" />
      </button>
      {open && (
        <ul className="rm-hist-list text-xs">
          {/* The latest repeats at the top of the trail on purpose: the list is
              the whole history, not "the rest of it". */}
          {events.map((e) => (
            <li key={e.id} className="flex-center">
              {line(e)}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
