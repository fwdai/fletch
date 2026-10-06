import { useState } from "react";
import { api, type ContextAssertion } from "@/api";
import { Button } from "@/components/ui/Button";
import { Modal, ModalBody, ModalFooter } from "@/components/ui/Modal";
import { TextArea } from "@/components/ui/TextInput";

/** Retract an assertion that was never right — distinct from changing one we
 *  changed our mind about. The reason is required. */
export function RetractDialog({
  assertion,
  projectId,
  onClose,
}: {
  assertion: ContextAssertion;
  projectId: string;
  onClose: () => void;
}) {
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = () => {
    setBusy(true);
    setError(null);
    api
      .contextRetract(projectId, assertion.id, reason.trim())
      .then(onClose)
      .catch((e) => {
        setError(String(e));
        setBusy(false);
      });
  };

  return (
    <Modal icon="trash" title="Retract this assertion" onClose={onClose}>
      <ModalBody className="pc-form">
        <p className="text-sm">{assertion.statement}</p>
        <p className="pc-meta text-xs">
          Retracting says it was never right. If the project changed its mind instead, use Change,
          which keeps the old one as history.
        </p>
        <div className="ps-field">
          <label className="ps-label text-sm" htmlFor="pc-retract-reason">
            Reason (required)
          </label>
          <TextArea
            id="pc-retract-reason"
            value={reason}
            onChange={(e) => setReason(e.target.value)}
          />
        </div>
        {error && <div className="pc-error text-sm">{error}</div>}
      </ModalBody>
      <ModalFooter>
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" danger disabled={!reason.trim() || busy} onClick={submit}>
          Retract
        </Button>
      </ModalFooter>
    </Modal>
  );
}
