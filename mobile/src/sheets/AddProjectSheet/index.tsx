import { HostPicker } from "../../components/HostPicker";
import { Segmented, Sheet } from "../../components/ui";
import { useHost } from "../../host/useHost";
import { severalHosts, useApp } from "../../store";
import { openSheetOn } from "../../store/hostNav";
import { CloneForm } from "./CloneForm";
import { OpenFolderForm } from "./OpenFolderForm";
import { useAddProject } from "./useAddProject";

const TABS = [
  { id: "folder", label: "Open folder" },
  { id: "clone", label: "Clone" },
];

/** The two ways a project reaches a host from the phone (docs/remote-protocol.md,
 *  "Adding a project from the phone"). Creating a brand-new repo is not one of
 *  them yet. With several hosts paired, the sheet says which one first — a
 *  folder is a folder on one Mac — and picking another moves the sheet there. */
export function AddProjectSheet({
  open,
  onClose,
  tab: initialTab,
}: {
  open: boolean;
  onClose: () => void;
  /** Which tab to open on; the folder browser unless the caller says `clone`. */
  tab?: string;
}) {
  const { hostKey } = useHost();
  const several = useApp(severalHosts);
  const { tab, switchTab, busy, error, setError, run } = useAddProject(
    open,
    initialTab === "clone" ? "clone" : "folder",
  );
  const moveTo = (to: string) => {
    setError(null);
    openSheetOn(to, "addProject", initialTab ? { tab: initialTab } : {});
  };

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
      {several && <HostPicker label="Add to" value={hostKey} onChange={moveTo} />}
      <div className="ap-seg">
        <Segmented items={TABS} value={tab} onChange={switchTab} />
      </div>
      {/* A form browses the host it is on, so another host starts it afresh. */}
      {tab === "folder" ? (
        <OpenFolderForm key={hostKey} busy={busy} error={error} setError={setError} run={run} />
      ) : (
        <CloneForm key={hostKey} busy={busy} error={error} setError={setError} run={run} />
      )}
    </Sheet>
  );
}
