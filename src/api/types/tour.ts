/** One place a walkthrough step points at: a file in the workspace's current
 *  diff and a 1-indexed, inclusive line range on its new side. */
export interface TourAnchor {
  path: string;
  start: number;
  end: number;
}

/** One step of the agent's walkthrough of its changes: a short note and the
 *  hunks it is about, which may span several files ("the button, its handler
 *  and its style" is one step, three anchors). */
export interface TourStep {
  title: string | null;
  note: string;
  anchors: TourAnchor[];
}

/** The agent-authored walkthrough of a workspace's changes, written through the
 *  `tour_set` mailbox op. Newest wins: a later turn's tour replaces the earlier
 *  one. `turn_id` is the user turn it was authored in, when the host knew it. */
export interface Tour {
  agent_id: string;
  turn_id: string | null;
  steps: TourStep[];
  /** Epoch ms. */
  created_at: number;
}

/** `tour:changed`: the workspace's tour was written; reload it with `get_tour`. */
export interface TourChangedEvent {
  agent_id: string;
}
