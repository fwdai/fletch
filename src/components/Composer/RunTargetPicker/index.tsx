import { useEffect, useState } from "react";
import type { WorkspaceKind } from "@/api";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore, useHostId } from "@/store";
import { type EnvironmentId, LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import { useWorkspaceTypeChip, WorkspaceTypePicker } from "../WorkspaceTypePicker";
import type { WorkspaceTypeChip } from "../workspaceType";
import { runTargetOptions } from "./options";
import { TargetMenu } from "./TargetMenu";
import { TargetPill } from "./TargetPill";
import { useProjectDefinition } from "./useProjectDefinition";

interface Props {
  /** The draft's repo on its own host, which This Mac's workspace types are
   *  read for. */
  repoPath: string;
  /** The draft's provider: it has to be able to run where the workspace
   *  does. */
  provider: string;
  /** The draft's project, read for whether it can run on another device. */
  projectId: string;
  /** The workspace type picked for This Mac, kept as clicked. */
  workspaceType?: WorkspaceKind;
  /** Where the workspace runs now (`draftRunsOn`). */
  runsOn: EnvironmentId;
  /** Only the draft's own host is offered: a roadmap hand-off. */
  pinned?: boolean;
  /** A place, and for This Mac the workspace type clicked with it. */
  onPick: (runsOn: EnvironmentId, workspaceType?: WorkspaceKind) => void;
}

/** Where a new workspace runs, on the new-workspace row beside the project and
 *  the branch: This Mac, in the workspace type picked for it, or a paired host
 *  by name. Scoped to the draft's own host, which is where it runs without a
 *  pick. With nowhere else to run it is the workspace-type pill as it always
 *  was on This Mac, and nothing on a paired host (whose spawn carries no
 *  workspace type). */
export function RunTargetPicker(props: Props) {
  const hostId = useHostId();
  const paired = useAppStore((s) => Object.keys(s.environments).length > 1);
  const local = hostId === LOCAL_ENVIRONMENT_ID;
  if (props.pinned || !paired) {
    return local ? (
      <WorkspaceTypePicker
        repoPath={props.repoPath}
        provider={props.provider}
        current={props.workspaceType}
        onPick={(kind) => props.onPick(hostId, kind)}
      />
    ) : null;
  }
  return local ? <WithWorkspaceTypes {...props} /> : <RunTargetControl {...props} types={null} />;
}

/** The control for a draft of This Mac's, with its workspace types read. */
function WithWorkspaceTypes(props: Props) {
  const { chip, load } = useWorkspaceTypeChip(props.repoPath, props.provider, props.workspaceType);
  return <RunTargetControl {...props} types={chip} loadTypes={load} />;
}

function RunTargetControl({
  provider,
  projectId,
  runsOn,
  onPick,
  types,
  loadTypes,
}: Props & { types: WorkspaceTypeChip; loadTypes?: () => void }) {
  const hostId = useHostId();
  const environments = useAppStore((s) => s.environments);
  const { definition, check } = useProjectDefinition(projectId);
  const [open, setOpen] = useState(false);
  const elsewhere = runsOn !== hostId;
  // A place other than the draft's own host is checked for whether the
  // project can go there as soon as it is picked, as well as on every open.
  useEffect(() => {
    if (elsewhere) check();
  }, [elsewhere, check]);

  const options = runTargetOptions({ environments, draftHost: hostId, provider, definition });
  const picked =
    options.find((o) => o.hostId === runsOn) ?? options.find((o) => o.hostId === hostId);
  if (!picked) return null;

  return (
    <span style={{ position: "relative" }}>
      <TargetPill
        option={picked}
        env={environments[picked.hostId]}
        types={picked.kind === "local" ? types : null}
        open={open}
        onClick={() => {
          if (!open) {
            check();
            loadTypes?.();
          }
          setOpen((v) => !v);
        }}
      />
      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <TargetMenu
            options={options}
            environments={environments}
            picked={picked}
            types={types}
            onPick={(target, workspaceType) => {
              onPick(target, workspaceType);
              setOpen(false);
            }}
          />
        </>
      )}
    </span>
  );
}
