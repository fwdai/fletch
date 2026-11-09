import { Button, Modal, ModalBody, ModalFooter } from "@/components/ui";
import { usePairBack } from "./usePairBack";

/** Asked once this Mac has paired with a host by confirmation, and only when
 *  this Mac is a host too: may that host control this Mac as well?
 *  (docs/remote-protocol.md, "Pairing back").
 *
 *  The answer is the person's here and nobody else's: nothing that reaches
 *  this Mac leaves it before Allow, so a host chosen by a name it merely
 *  announced gets nothing it was not given on purpose. */
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
          Also let “{offer.name}” control this Mac? It gets full access, like a device paired here
          with Full.
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
