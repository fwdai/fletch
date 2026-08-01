import { open as openExternal } from "@tauri-apps/plugin-shell";
import { Icon, type IconName } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { useAppStore } from "@/store";
import { type BoardItem, type ItemSource, type ItemStatus, SIZE_HINT } from "../types";

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
    // is the difference between "check back here" and "go merge it".
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
  /** Hand the item to the queue (`open → queued`). Absent for a ghost and on a
   *  read-only board. */
  onQueue?: () => void;
  /** Take it back off the queue before it's dispatched (`queued → open`). */
  onUnqueue?: () => void;
  /** Open the run this item is being built by. Only on an item with a run. */
  onOpenRun?: () => void;
  /** The workflow this item would run under ("Project default" resolved), or
   *  null when nothing would run it — the queue would stall on it. */
  workflowName?: string | null;
  /** Why this item isn't moving, straight from the queue: the drainer's reason
   *  a queued row is stuck, or the merge sweep's "PR #N was closed without
   *  merging" for one that came back off review. */
  note?: string;
  /** Ring the row: it was just jumped to, or a pending proposal moves it. */
  focused?: boolean;
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

/** One roadmap row: a click-to-expand header line (code, title, size, source)
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
  onQueue,
  onUnqueue,
  onOpenRun,
  workflowName,
  note,
  cardRef,
}: Props) {
  const createDraft = useAppStore((s) => s.createDraft);
  const closeProjectScreen = useAppStore((s) => s.closeProjectScreen);
  const source = SOURCE[item.source];
  const state = STATE[item.status];
  const cls = [
    "rm-item",
    ghost ? "ghost" : "",
    open ? "open" : "",
    landed ? "landed" : "",
    focused ? "focus" : "",
    item.status === "queued" ? "queued" : "",
  ]
    .filter(Boolean)
    .join(" ");

  return (
    <div ref={cardRef} className={cls}>
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
        {item.epic && <span className="rm-epic text-xs">{item.epic}</span>}
        {/* A dispatched item shows the pearl whether or not an agent id has
            been stamped on it yet: the queue flips it to `active` at the moment
            it claims the row, a beat before the run exists. */}
        {item.status === "active" && (
          <span className="rm-live iflex-center mono text-xs">
            <span className="rm-pearl" />
            {item.agent ?? "running"}
          </span>
        )}
        {state && (
          <span className={`rm-state iflex-center text-xs st-${state.cls}`} title={state.tip}>
            {state.label}
          </span>
        )}
        {item.size && (
          <span
            className={`rm-size iflex-center mono text-xs s-${item.size}`}
            title={SIZE_HINT[item.size]}
          >
            {item.size}
          </span>
        )}
        <span className={`rm-src iflex-center src-${item.source}`} title={source.tip}>
          <Icon name={source.icon} size={10} />
        </span>
        <Icon name="chevD" size={10} className="rm-chev" />
      </button>

      {/* The two buttons that decide a proposal's fate. Outside the header
          button (no nesting) and outside the collapsible body, so ruling on a
          ghost never costs an expand — reading it first is what the expand is
          for. */}
      {ghost && (onAccept || onDiscard) && (
        <div className="rm-ghostbar flex-center">
          <span className="rm-ghostbar-l text-xs">Proposed — not on the roadmap yet</span>
          <span className="grow" />
          {onDiscard && (
            <Button variant="ghost" size="sm" onClick={onDiscard}>
              Discard
            </Button>
          )}
          {onAccept && (
            <Button variant="primary" size="sm" onClick={onAccept}>
              <Icon name="check" size={11} /> Accept
            </Button>
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
          {item.why && <p className="rm-why text-sm">{item.why}</p>}
          {item.accept && (
            <ul className="rm-accept text-sm">
              {item.accept.map((a) => (
                <li key={a}>{a}</li>
              ))}
            </ul>
          )}
          <div className="rm-item-foot flex-center">
            {item.area && <span className="rm-area mono text-xs">{item.area}</span>}
            {item.deps?.map((d) => (
              <span key={d} className="rm-dep iflex-center mono text-xs">
                <Icon name="arrowR" size={9} />
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
                <Icon name="combine" size={9} />
                {workflowName ?? "no workflow"}
              </span>
            )}
            <span className="grow" />
            {onEdit && (
              <Button variant="ghost" size="sm" onClick={onEdit}>
                <Icon name="edit" size={11} /> Edit
              </Button>
            )}
            {/* The manual hand-off stays available on every real row: the queue
                is autonomous, and sometimes you want to drive. Demoted to a
                ghost button next to "Queue", which is the path most rows take.
                A proposed row isn't work anyone has agreed to do — accept it
                first, then send it. */}
            {!ghost && (
              <Button
                variant="ghost"
                size="sm"
                onClick={async () => {
                  const draftId = await createDraft(repoPath, briefFor(item));
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
            {onQueue && (
              <Button variant="primary" size="sm" onClick={onQueue}>
                <Icon name="play" size={11} /> Queue
              </Button>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
