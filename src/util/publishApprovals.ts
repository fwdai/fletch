// Folding the events that raced an `approvals_list` back over its answer.
//
// The host forwards events and writes op responses from separate tasks, so
// there is no order between them: a `publish:approval-requested` raised just
// after the list was read can reach the client BEFORE the list does. Replacing
// the queue wholesale then drops that card until the next handshake — the host
// stays blocked and nothing on screen says so. The mirror case is worse the
// other way: a `publish:approval-resolved` that arrives first is undone by the
// snapshot, putting back a card for a question that is over.
//
// So a client buffers the approval events that land while a list is in flight
// and replays them over the snapshot when it arrives. The boundary is the same
// on both clients, so it lives here beside the other helpers mobile imports
// through `@desktop` (util/attachmentUpload, util/paths).

import type { PublishApproval } from "../api/types/sandbox";

/** An approval event that landed while a list was in flight, in the order it
 *  landed. */
export type ApprovalEvent =
  | { kind: "requested"; request: PublishApproval }
  | { kind: "resolved"; id: string };

/** The queue as it stands: what the host said it was blocked on, plus what it
 *  has said since. A request already in the snapshot is not added twice — the
 *  event and the list can both carry it. */
export function replayApprovalEvents(
  snapshot: PublishApproval[],
  events: ApprovalEvent[],
): PublishApproval[] {
  let queue = snapshot;
  for (const event of events) {
    if (event.kind === "resolved") {
      queue = queue.filter((r) => r.id !== event.id);
    } else if (!queue.some((r) => r.id === event.request.id)) {
      queue = [...queue, event.request];
    }
  }
  return queue;
}
