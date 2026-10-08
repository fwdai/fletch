import { Button, Modal, ModalBody, ModalFooter } from "@/components/ui";
import { usePairRequest } from "./usePairRequest";
import "./PairRequest.css";

const PLATFORM_LABELS: Record<string, string> = { ios: "iPhone", macos: "Mac", android: "phone" };

/** A device on the network asking this Mac to pair (docs/remote-protocol.md,
 *  "Confirmed pairing"). Only ever shown while "Pair a device" is open — the
 *  host refuses requests outside that window — and app-wide, so the answer
 *  does not depend on which screen the person is on.
 *
 *  The code is the check: the device shows the same six digits, and they
 *  match only if nothing sits between the two. Dismissing declines — for a
 *  pairing, "no" is the safe default, and the phone can simply ask again. */
export function PairRequestPrompt() {
  const { request, answer } = usePairRequest();
  if (!request) return null;
  const kind = PLATFORM_LABELS[request.platform] ?? "device";

  return (
    <Modal
      icon="laptop"
      title={`Pair “${request.deviceName}”?`}
      size="sm"
      layer="overlay"
      onClose={() => answer(false)}
    >
      <ModalBody>
        <p className="text-base">
          This {kind} wants to control the agents on this Mac. Accept only if it shows this code:
        </p>
        <div className="pair-request-code mono">
          {request.code.slice(0, 3)} {request.code.slice(3)}
        </div>
      </ModalBody>
      <ModalFooter>
        <Button variant="ghost" onClick={() => answer(false)}>
          Decline
        </Button>
        <Button variant="primary" onClick={() => answer(true)}>
          Accept
        </Button>
      </ModalFooter>
    </Modal>
  );
}
