import { Button, Modal, ModalBody, ModalFooter } from "@/components/ui";
import { useAppStore } from "@/store";

/** Approval prompt for an agent's publish — shown only when the
 *  `publish_confirmation` setting is on.
 *
 *  Credentials never enter an agent's sandbox and it already cannot push the
 *  branch its work is reviewed against (`rpc::caps`). This is the remaining gate:
 *  whether publishing under the user's identity is something they approved.
 *
 *  Shows the head of the queue only. Two agents can be waiting at once, and
 *  stacking prompts would make it ambiguous which one a click answers; the next
 *  surfaces as soon as this is answered.
 *
 *  Declining is not an error state — an agent that is told no reports the refusal
 *  and carries on — so the prompt has no "cancel" distinct from "don't publish".
 *  Dismissing (scrim, Escape) deliberately does nothing: the backend denies
 *  unanswered requests after its own timeout, so a stray click cannot approve,
 *  and leaving the prompt up keeps the decision visible.
 */
export function PublishApproval() {
  const pending = useAppStore((s) => s.pendingPublishApprovals);
  const answer = useAppStore((s) => s.answerPublishApproval);
  const request = pending[0];
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
          An agent wants to <strong>{request.detail}</strong> using your GitHub identity.
        </p>
        {pending.length > 1 && (
          <p className="text-xs">
            {pending.length - 1} more {pending.length === 2 ? "request" : "requests"} waiting.
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
