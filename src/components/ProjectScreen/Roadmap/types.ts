// The Roadmap surface's data model. Everything here is currently fed by
// `mockData.ts` — there is no roadmap table or API yet. The shapes are written
// as if they came from the DB (flat rows, string codes, explicit enums) so the
// eventual schema can be modelled straight off them.

import type { UIAnswer, UIQuestion } from "@/components/Workspace/messages/UserInput/parse";

/** Where an item sits on the board. `now` is being built, `next` is queued,
 *  `later` is the backlog. Shipped items leave the board (see `shipped`). */
export type Horizon = "now" | "next" | "later";

export type ItemSize = "XS" | "S" | "M" | "L";

/** Where the item came from — drawn as a glyph on the row. */
export type ItemSource = "pm" | "linear" | "github";

/** `active` means an agent is on it right now. */
export type ItemStatus = "open" | "active";

export interface RoadmapItem {
  /** Short human id ("FLT-142", "#207") — unique per project. */
  code: string;
  title: string;
  horizon: Horizon;
  size: ItemSize;
  /** Product-map domain this belongs to (`MapDomain.id`). */
  area: string;
  source: ItemSource;
  status: ItemStatus;
  /** Why it's on the board — the one line that justifies its place. */
  why: string;
  /** Acceptance criteria, rendered as a checklist. */
  accept?: string[];
  /** Codes this item must land after. */
  deps?: string[];
  /** Optional grouping label when several items were shaped together. */
  epic?: string;
  /** Agent working it — only meaningful while `status === "active"`. */
  agent?: string;
  /** Transient highlight for a row that just landed on the board. */
  justAdded?: boolean;
}

/** A slice of the codebase the PM agent knows about, shown on the Product map
 *  tab. `heat` is how much recent work has touched it. */
export interface MapDomain {
  id: string;
  label: string;
  note: string;
  files: number;
  /** Roadmap items currently pointing at this domain. */
  items: number;
  heat: "hot" | "warm" | "cool";
}

// ── the PM conversation ──────────────────────────────────────────────

/** One line of the repo check the PM runs before writing anything down.
 *  `ok` — capability found, `warn` — nothing covers it / a hazard,
 *  `dep` — it links to something already on the board. */
export type FindingKind = "ok" | "warn" | "dep";

export interface Finding {
  kind: FindingKind;
  /** May contain `backticked` spans, rendered as inline code. */
  text: string;
}

/** A single edit the PM proposes to the board. Nothing is applied until the
 *  user accepts the proposal that carries it. */
export type ProposalChange =
  | { kind: "add"; item: RoadmapItem }
  | { kind: "move"; code: string; from: Horizon; to: Horizon; why: string };

/** The follow-up the PM plays once a question is answered. */
export interface AnsweredBeat {
  text: string;
  note: string;
  changes: ProposalChange[];
}

/** A thread message without its id — the shape mock data and the runtime
 *  script are authored in. `useRoadmap` stamps an id on push. */
export type PmBody =
  | { kind: "user"; body: string }
  | { kind: "text"; body: string }
  | { kind: "thinking"; body: string }
  | { kind: "probe"; summary: string; findings: Finding[] }
  | { kind: "question"; question: UIQuestion; answered: AnsweredBeat; answer?: UIAnswer | null }
  | {
      kind: "proposal";
      note: string;
      changes: ProposalChange[];
      resolved?: "accepted" | "discarded" | null;
    }
  | { kind: "landed"; codes: string[] };

export type PmMessage = { id: string } & PmBody;

/** A canned exchange: what the user says, and what the PM plays back. */
export interface ScriptBeat {
  prompt: string;
  msgs: PmBody[];
}

export const SIZE_HINT: Record<ItemSize, string> = {
  XS: "a few minutes",
  S: "under an hour",
  M: "half a day",
  L: "multi-day",
};

/** Board groups, in display order. The same labels drive the header stats. */
export const HORIZONS: { id: Horizon; label: string; note: string }[] = [
  { id: "now", label: "In flight", note: "being built" },
  { id: "next", label: "Next", note: "queued" },
  { id: "later", label: "Later", note: "backlog" },
];

export const FINDING_TAG: Record<FindingKind, string> = {
  ok: "found",
  warn: "watch",
  dep: "links",
};

export const HEAT_LABEL: Record<MapDomain["heat"], string> = {
  hot: "active",
  warm: "planned",
  cool: "quiet",
};
