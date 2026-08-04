// Board/ProductBrief.tsx — the product's memory, as the user reads and rules it.
//
// This tab used to be the "Product map": five hardcoded domains with a lede
// claiming they were what the PM knew about the codebase. They weren't — nothing
// produced them — so the one surface that promised memory was the one thing on
// this screen that had none. Now it renders the real brief
// (src-tauri/src/roadmap/memory.rs): the document the PM is spawned with, and the
// only place the *why* of the product lives.
//
// Two things on one tab, in decision-first order, because that is the board's
// grammar everywhere else: a pending ask is ruled on above the state it would
// replace. The ask carries the whole proposed document (never a diff — the user
// is accepting the brief they will get), collapsible so the tab still opens as a
// reading surface rather than a wall of two documents.
//
// Markdown through the shared renderer, deliberately without a `TokenChipContext`:
// item codes are the *board's* vocabulary, and a brief that linkified them would
// be quietly encouraging exactly the thing the PM is told not to write here.

import { useState } from "react";
import type { RoadmapBrief, RoadmapBriefProposal } from "@/api";
import { Icon } from "@/components/Icon";
import { Markdown } from "@/components/Markdown";
import { formatAge } from "@/util/format";
import { DecisionBar } from "./DecisionBar";

/** What the ask's bar says, which depends on whether there is anything to
 *  replace: the first brief is a draft to admit, a later one is a revision of a
 *  document the user already agreed to. Pure so the wording is pinned by a test
 *  rather than by reading the JSX. */
export function briefAskLabel(hasBrief: boolean): string {
  return hasBrief ? "PM proposes a new product brief" : "PM drafted the first product brief";
}

export function ProductBrief({
  brief,
  proposal,
  onAccept,
  onDecline,
}: {
  /** The standing brief, or null when the PM has never written one. */
  brief: RoadmapBrief | null;
  /** The PM's pending ask to replace it, or null. */
  proposal: RoadmapBriefProposal | null;
  /** Absent on a read-only board — the bar then isn't rendered at all. */
  onAccept?: () => void;
  onDecline?: () => void;
}) {
  // Open by default: ruling on a document you haven't read is the one thing this
  // preview exists to prevent.
  const [open, setOpen] = useState(true);
  // A read-only board hands neither action, and a decision surface with no
  // decision on it is furniture — so the ask simply isn't drawn there, exactly as
  // for a card's proposal bar.
  const ask = onAccept != null || onDecline != null ? proposal : null;

  return (
    <div className="rm-brief">
      {ask && (
        <div className="rm-brief-ask">
          <DecisionBar
            label={briefAskLabel(brief != null)}
            note={ask.note}
            variant="prop"
            declineLabel="Decline"
            onAccept={onAccept}
            onDecline={onDecline}
          />
          <button
            type="button"
            className="rm-brief-t flex-center text-xs"
            onClick={() => setOpen((v) => !v)}
            aria-expanded={open}
          >
            <Icon name="chevD" size={9} className={`rm-brief-chev ${open ? "open" : ""}`} />
            <span>{open ? "Hide the proposed brief" : "Read the proposed brief"}</span>
          </button>
          {open && (
            <div className="rm-brief-md m-agent text-sm">
              <Markdown>{ask.content}</Markdown>
            </div>
          )}
        </div>
      )}

      {brief ? (
        <>
          <div className="rm-brief-h flex-center text-xs">
            <Icon name="notebookPen" size={11} />
            <span className="rm-brief-l">
              {ask ? "Current brief" : "What the PM knows about this product"}
            </span>
            <span className="grow" />
            <span className="rm-brief-age mono">
              updated {formatAge(brief.updated_at, Date.now())}
            </span>
          </div>
          <div className="rm-brief-md m-agent text-sm">
            <Markdown>{brief.content}</Markdown>
          </div>
        </>
      ) : (
        // Nothing to read, and — unlike the old mock — nothing pretending
        // otherwise. The way out is the conversation on the left, because the PM
        // is the only writer: the user rules on a brief, they don't type one.
        !ask && (
          <div className="rm-blank">
            <span className="rm-blank-badge iflex-center">
              <Icon name="notebookPen" size={18} />
            </span>
            <h3 className="rm-blank-h text-base">No product brief yet</h3>
            <p className="rm-blank-b text-sm">
              This is where the project manager keeps what it knows about the product — the vision,
              the domains, the constraints, and the directions you've ruled out. It reads it back at
              the start of every session, so a decision made here doesn't have to be made twice.
            </p>
            <p className="rm-blank-b text-sm">
              Ask the PM to draft the product brief. It proposes; you accept.
            </p>
          </div>
        )
      )}
    </div>
  );
}
