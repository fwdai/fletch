import { useState } from "react";
import { api, type ContextAssertion } from "@/api";
import { Button } from "@/components/ui/Button";
import { Modal, ModalBody, ModalFooter } from "@/components/ui/Modal";
import { TextArea } from "@/components/ui/TextInput";
import type { Tension } from "./format";

/** Rule on a contradiction: close the edge with the reasoning the log
 *  requires. Neither assertion changes — Change or Retract do that. */
export function ResolveDialog({
  assertion,
  tension,
  projectId,
  onClose,
}: {
  assertion: ContextAssertion;
  tension: Tension;
  projectId: string;
  onClose: () => void;
}) {
  const [reasoning, setReasoning] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = () => {
    setBusy(true);
    setError(null);
    api
      .contextResolveContradiction(projectId, tension.edge.a, tension.edge.b, reasoning.trim())
      .then(onClose)
      .catch((e) => {
        setError(String(e));
        setBusy(false);
      });
  };

  return (
    <Modal icon="check" title="Resolve this contradiction" onClose={onClose}>
      <ModalBody className="pc-form">
        <p className="text-sm">{assertion.statement}</p>
        <p className="text-sm">{tension.other?.statement ?? tension.otherId}</p>
        {tension.edge.reasoning && <p className="pc-meta text-xs">{tension.edge.reasoning}</p>}
        <p className="pc-meta text-xs">
          Resolving records that both can stand. If one of them is wrong, retract it; if the project
          changed its mind, use Change.
        </p>
        <div className="ps-field">
          <label className="ps-label text-sm" htmlFor="pc-resolve-reasoning">
            Reasoning (required)
          </label>
          <TextArea
            id="pc-resolve-reasoning"
            value={reasoning}
            onChange={(e) => setReasoning(e.target.value)}
          />
        </div>
        {error && <div className="pc-error text-sm">{error}</div>}
      </ModalBody>
      <ModalFooter>
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" disabled={!reasoning.trim() || busy} onClick={submit}>
          Resolve
        </Button>
      </ModalFooter>
    </Modal>
  );
}
