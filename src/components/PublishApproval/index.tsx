import { Button, Modal, ModalBody, ModalFooter } from "@/components/ui";
import { useAppStore, useEveryHost } from "@/store";
import { useHostWhere } from "@/store/capabilities";
import { EngineScope } from "@/store/EngineScope";

/** Approval prompt for an agent's publish — shown only when the
 *  `publish_confirmation` setting is on.
 *
 *  Credentials never enter an agent's sandbox and it already cannot push the
 *  branch its work is reviewed against (`rpc::caps`). This is the remaining gate:
 *  whether publishing under the user's identity is something they approved.
 *
 *  Any host's agent can ask, whichever host is on screen, so the prompt shows
 *  the head of every host's queue in turn (This Mac's first) and is answered
 *  in that host's scope, against the engine that is waiting. One prompt at a
 *  time: two agents can be waiting at once, and stacking prompts would make it
 *  ambiguous which one a click answers; the next surfaces as soon as this is
 *  answered.
 *
 *  Declining is not an error state — an agent that is told no reports the refusal
 *  and carries on — so the prompt has no "cancel" distinct from "don't publish".
 *  Dismissing (scrim, Escape) deliberately does nothing: the backend denies
 *  unanswered requests after its own timeout, so a stray click cannot approve,
 *  and leaving the prompt up keeps the decision visible.
 */
export function PublishApproval() {
  const hostIds = useEveryHost((_s, hostId) => hostId);
  const waiting = useEveryHost((s) => s.pendingPublishApprovals.length);
  const at = waiting.findIndex((n) => n > 0);
  if (at < 0) return null;
  const total = waiting.reduce((sum, n) => sum + n, 0);

  return (
    <EngineScope hostId={hostIds[at]}>
      <ApprovalPrompt more={total - 1} />
    </EngineScope>
  );
}

/** The head of the scope's host's queue, with `more` waiting behind it across
 *  every host. */
function ApprovalPrompt({ more }: { more: number }) {
  const request = useAppStore((s) => s.pendingPublishApprovals[0]);
  const answer = useAppStore((s) => s.answerPublishApproval);
  const where = useHostWhere();
  if (!request) return null;

  return (
    <Modal
      icon="github"
      title="Approve publishing?"
      layer="overlay"
      onClose={() => {
        /* see the component doc: dismissal must not answer */
      }}
    >
      <ModalBody>
        <p className="text-base">
          An agent{where} wants to{" "}
          <strong>{request.detail}</strong> using your GitHub identity.
        </p>
        {more > 0 && (
          <p className="text-xs">
            {more} more {more === 1 ? "request" : "requests"} waiting.
          </p>
        )}
      </ModalBody>
      <ModalFooter>
        <Button variant="ghost" onClick={() => answer(request.id, false)}>
          Don't publish
        </Button>
        <Button variant="primary" onClick={() => answer(request.id, true)}>
          Publish
        </Button>
      </ModalFooter>
    </Modal>
  );
}
