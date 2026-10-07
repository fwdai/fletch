import { useState } from "react";
import { api, type ContextAssertion, type Stance } from "@/api";
import { Button } from "@/components/ui/Button";
import { Modal, ModalBody, ModalFooter } from "@/components/ui/Modal";
import { TextArea } from "@/components/ui/TextInput";

/** "Change" an assertion: records a new one that supersedes it, with the
 *  reasoning the log requires. The old one stays, as history. */
export function SupersedeDialog({
  assertion,
  projectId,
  onClose,
}: {
  assertion: ContextAssertion;
  projectId: string;
  onClose: () => void;
}) {
  const [statement, setStatement] = useState(assertion.statement);
  const [rationale, setRationale] = useState(assertion.rationale);
  const [stance, setStance] = useState<Stance>(assertion.stance);
  const [reasoning, setReasoning] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ready = statement.trim() && reasoning.trim() && !busy;

  const submit = () => {
    setBusy(true);
    setError(null);
    api
      .contextRecordAssertion(projectId, {
        kind: assertion.kind,
        domain: assertion.domain,
        stance,
        statement: statement.trim(),
        rationale: rationale.trim(),
        paths: assertion.paths,
        about: assertion.about,
        supersedes: { id: assertion.id, reasoning: reasoning.trim() },
        contradicts: [],
        status: "confirmed",
      })
      .then(onClose)
      .catch((e) => {
        setError(String(e));
        setBusy(false);
      });
  };

  return (
    <Modal icon="edit" title="Change this decision" onClose={onClose} size="lg">
      <ModalBody className="pc-form">
        <div className="ps-field">
          <label className="ps-label text-sm" htmlFor="pc-sup-statement">
            New statement
          </label>
          <TextArea
            id="pc-sup-statement"
            value={statement}
            onChange={(e) => setStatement(e.target.value)}
          />
        </div>
        <div className="ps-field">
          <label className="ps-label text-sm" htmlFor="pc-sup-rationale">
            Rationale
          </label>
          <TextArea
            id="pc-sup-rationale"
            value={rationale}
            onChange={(e) => setRationale(e.target.value)}
          />
        </div>
        <div className="ps-field">
          <label className="ps-label text-sm" htmlFor="pc-sup-stance">
            Stance
          </label>
          <select
            id="pc-sup-stance"
            className="ps-input text-sm"
            value={stance}
            onChange={(e) => setStance(e.target.value as Stance)}
          >
            <option value="adopted">adopted</option>
            <option value="rejected">rejected</option>
          </select>
        </div>
        <div className="ps-field">
          <label className="ps-label text-sm" htmlFor="pc-sup-reasoning">
            Why it changed (required)
          </label>
          <TextArea
            id="pc-sup-reasoning"
            value={reasoning}
            placeholder="What we learned, or what moved"
            onChange={(e) => setReasoning(e.target.value)}
          />
        </div>
        {error && <div className="pc-error text-sm">{error}</div>}
      </ModalBody>
      <ModalFooter>
        <Button variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button variant="primary" disabled={!ready} onClick={submit}>
          Record change
        </Button>
      </ModalFooter>
    </Modal>
  );
}
