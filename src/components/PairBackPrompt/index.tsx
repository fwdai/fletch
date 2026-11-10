import { Button, Modal, ModalBody, ModalFooter } from "@/components/ui";
import { usePairBack } from "./usePairBack";
import "./PairBackPrompt.css";

/** Asked once this Mac has paired with a host by confirmation: may that host
 *  control this Mac as well? (docs/remote-protocol.md, "Pairing back").
 *
 *  The answer is the person's here and nobody else's: nothing that reaches
 *  this Mac leaves it before Allow, so a host chosen by a name it merely
 *  announced gets nothing it was not given on purpose. The second line is the
 *  check that makes Allow safe: a host that accepted its own prompt showed
 *  nobody anything. */
export function PairBackPrompt() {
  const { offer, sending, error, allow, dismiss } = usePairBack();
  if (!offer) return null;

  return (
    <Modal
      icon="laptop"
      title={`Paired with “${offer.name}”`}
      size="sm"
      layer="overlay"
      onClose={dismiss}
    >
      <ModalBody>
        <p className="text-base">
          Let “{offer.name}” control this Mac too? It gets full access to the agents here.
          <span className="pair-back-note text-sm">
            Allow only if you accepted this pairing on “{offer.name}” yourself.
            {offer.enables && " This turns on remote control for this Mac."}
          </span>
        </p>
        {error && <div className="modal-error text-sm">{error}</div>}
      </ModalBody>
      <ModalFooter>
        <Button variant="ghost" onClick={dismiss} disabled={sending}>
          Not now
        </Button>
        <Button variant="primary" onClick={allow} disabled={sending}>
          {sending ? "Allowing…" : "Allow"}
        </Button>
      </ModalFooter>
    </Modal>
  );
}
