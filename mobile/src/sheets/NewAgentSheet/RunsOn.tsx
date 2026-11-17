import { preparingLabel } from "@desktop/components/Composer/RunTargetPicker/options";
import { HostPicker } from "../../components/HostPicker";
import { ignore } from "../../lib/ignore";
import { runsOnReason } from "../../lib/runsOn";
import { severalHosts, useApp, useRunsOnHosts } from "../../store";
import type { ProjectChoice } from "../useProjectChoice";

/** Which host the sheet's agent runs on, said once more than one is paired:
 *  the sheet's own, picked, beside the others that could run it (lib/runsOn).
 *  Picking one moves the sheet there (`setHostKey`), by way of a copy of the
 *  project for a host that lacks it, which a line under the row names while it
 *  is being made. */
export function RunsOn({ choice }: { choice: ProjectChoice }) {
  const several = useApp(severalHosts);
  const hosts = useRunsOnHosts();
  const preparing = useApp((s) => s.hosts.find((h) => h.hostKey === choice.preparing)?.name);
  const { entry } = choice;
  if (!several || !entry) return null;

  const reasonOf = (hostKey: string) => {
    const target = hosts.find((h) => h.hostKey === hostKey);
    return target ? runsOnReason(entry, target, hosts) : null;
  };
  return (
    <>
      <HostPicker
        label="Runs on"
        value={choice.hostKey}
        // A failure is in `lastError`, which the sheet shows.
        onChange={(to) => void choice.setHostKey(to).catch(ignore)}
        reasonOf={reasonOf}
      />
      {preparing && <div className="chips-note host-note">{preparingLabel(preparing)}</div>}
    </>
  );
}
