import { useState } from "react";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore, useHostId } from "@/store";
import type { EnvironmentId } from "@/store/environments";
import { EnvironmentTab } from "../EnvironmentFrame";
import { runTargetOptions } from "./options";
import { TargetMenu } from "./TargetMenu";
import { useProjectDefinition } from "./useProjectDefinition";

interface Props {
  /** The draft's project, read for whether it can run on another device. */
  projectId: string;
  /** Only the draft's own host is offered: a roadmap hand-off. */
  pinned?: boolean;
  /** A place other than the draft's host: where the draft is to move. */
  onPick: (host: EnvironmentId) => void;
}

/** Which machine a new workspace runs on, on the sheet behind the composer's
 *  top edge (`EnvironmentFrame`, which draws it only with a host paired): This
 *  Mac or a paired host by name. Scoped to the draft's host, which is where it
 *  runs and so the place shown as picked. Which sandbox it runs in there is
 *  not the sheet's: the new-workspace row picks it (`WorkspaceTypePicker`). */
export function RunTargetPicker({ projectId, pinned, onPick }: Props) {
  const hostId = useHostId();
  const environments = useAppStore((s) => s.environments);
  const { definition, check } = useProjectDefinition(projectId);
  const [open, setOpen] = useState(false);

  const places = runTargetOptions({ environments, draftHost: hostId, definition });
  const options = pinned ? places.filter((o) => o.hostId === hostId) : places;
  const picked = options.find((o) => o.hostId === hostId);
  const env = picked && environments[picked.hostId];
  if (!picked || !env) return null;

  // Nowhere else to go: the tab only says where the workspace runs.
  if (options.length === 1) return <EnvironmentTab env={env} />;

  return (
    <span style={{ position: "relative", display: "flex", minWidth: 0 }}>
      <EnvironmentTab
        env={env}
        menu={{
          open,
          onToggle: () => {
            if (!open) check();
            setOpen((v) => !v);
          },
        }}
      />
      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <TargetMenu
            options={options}
            environments={environments}
            picked={picked}
            onPick={(target) => {
              if (target !== hostId) onPick(target);
              setOpen(false);
            }}
          />
        </>
      )}
    </span>
  );
}
