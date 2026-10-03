import { createPortal } from "react-dom";
import type { RestoreReport } from "@/api";
import { Button, Modal, ModalBody, ModalFooter } from "@/components/ui";
import { type CodeScope, restoreConfirmation } from "./confirm";

/** The confirmation before a rewind restores the code: the commits each
 *  branch loses, flagged when already pushed, every checkout that stays as it
 *  is for want of a snapshot, and where uncommitted changes go. Portaled to
 *  the body, since it opens from a message's hover-revealed actions, which
 *  fade out from under it once the pointer leaves. */
export function ConfirmRestore({
  scope,
  report,
  onConfirm,
  onCancel,
}: {
  scope: CodeScope;
  report: RestoreReport;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const confirmation = restoreConfirmation(scope, report);
  return createPortal(
    <Modal icon="rewind" title={confirmation.title} onClose={onCancel}>
      <ModalBody>
        <p className="text-base">{confirmation.summary}</p>
        {confirmation.keptAsIs.length > 0 && (
          <ul className="rewind-commits text-sm">
            {confirmation.keptAsIs.map((row) => (
              <li key={row}>{row}</li>
            ))}
          </ul>
        )}
        {confirmation.changes.length === 0 && (
          <p className="text-base">No commits leave a branch.</p>
        )}
        {confirmation.changes.map((change) => (
          <div key={change.subdir} className="rewind-change">
            <p className="text-base">
              {commits(change.leaving.length)} leave <code>{change.branch ?? "HEAD"}</code> in{" "}
              {change.subdir}:
            </p>
            <ul className="rewind-commits text-sm">
              {change.leaving.map((commit) => (
                <li key={commit.sha}>
                  <code className="rewind-sha">{commit.sha.slice(0, 7)}</code> {commit.subject}
                  {commit.pushed && <span className="rewind-pushed text-xs">pushed</span>}
                </li>
              ))}
            </ul>
          </div>
        ))}
        {confirmation.pushed && (
          <p className="modal-error text-sm">
            Some of these commits were already pushed: your next push will need to force.
          </p>
        )}
        <p className="text-sm">
          What each restored checkout holds now, uncommitted changes included, is kept as an undo
          point until the agent's next turn.
        </p>
      </ModalBody>
      <ModalFooter>
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button variant="primary" onClick={onConfirm}>
          {confirmation.action}
        </Button>
      </ModalFooter>
    </Modal>,
    document.body,
  );
}

function commits(n: number): string {
  return n === 1 ? "1 commit" : `${n} commits`;
}
