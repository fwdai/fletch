import { Segmented, Sheet } from "../../components/ui";
import { CloneForm } from "./CloneForm";
import { OpenFolderForm } from "./OpenFolderForm";
import { useAddProject } from "./useAddProject";

const TABS = [
  { id: "folder", label: "Open folder" },
  { id: "clone", label: "Clone" },
];

/** The two ways a project reaches the host from the phone (docs/remote-protocol.md,
 *  "Adding a project from the phone"). Creating a brand-new repo is not one of
 *  them yet. */
export function AddProjectSheet({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { tab, switchTab, busy, error, setError, run } = useAddProject(open);

  return (
    <Sheet
      open={open}
      onClose={onClose}
      full
      title="Add project"
      left={
        <button type="button" className="tbtn q" onClick={onClose}>
          Cancel
        </button>
      }
    >
      <div className="ap-seg">
        <Segmented items={TABS} value={tab} onChange={switchTab} />
      </div>
      {tab === "folder" ? (
        <OpenFolderForm busy={busy} error={error} run={run} />
      ) : (
        <CloneForm busy={busy} error={error} setError={setError} run={run} />
      )}
    </Sheet>
  );
}
