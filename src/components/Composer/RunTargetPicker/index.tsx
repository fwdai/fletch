import { useState } from "react";
import type { WorkspaceKind } from "@/api";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore, useHostId } from "@/store";
import { useHasPairedHost } from "@/store/capabilities";
import { type EnvironmentId, LOCAL_ENVIRONMENT_ID } from "@/store/environments";
import { EnvironmentTab } from "../EnvironmentFrame";
import { useWorkspaceTypeChip, WorkspaceTypePicker } from "../WorkspaceTypePicker";
import { type WorkspaceTypeChip, workspaceTypeLabel } from "../workspaceType";
import { runTargetOptions } from "./options";
import { TargetMenu } from "./TargetMenu";
import { useProjectDefinition } from "./useProjectDefinition";

interface Props {
  /** The draft's repo and provider, which This Mac's workspace types are
   *  read for. */
  repoPath: string;
  provider: string;
  /** The draft's project, read for whether it can run on another device. */
  projectId: string;
  /** The workspace type picked for This Mac, kept as clicked. */
  workspaceType?: WorkspaceKind;
  /** Only the draft's own host is offered: a roadmap hand-off. */
  pinned?: boolean;
  /** A place, and for This Mac the workspace type clicked with it. A place
   *  other than the draft's host is where the draft is to move. */
  onPick: (host: EnvironmentId, workspaceType?: WorkspaceKind) => void;
}

/** Where a new workspace runs, on the new-workspace row beside the project and
 *  the branch, for a window with no paired host: the workspace-type pill as it
 *  always was. With a host paired the row has nothing to say about it: the tab
 *  on the composer's top edge does ([`RunTargetTab`]). */
export function RunTargetPicker(props: Props) {
  const paired = useHasPairedHost();
  const local = useHostId() === LOCAL_ENVIRONMENT_ID;
  if (paired || !local) return null;
  return (
    <WorkspaceTypePicker
      repoPath={props.repoPath}
      provider={props.provider}
      current={props.workspaceType}
      onPick={(kind) => props.onPick(LOCAL_ENVIRONMENT_ID, kind)}
    />
  );
}

/** Where a new workspace runs, on the tab at the composer's top edge
 *  (`EnvironmentFrame`, which draws it only with a host paired): This Mac, in
 *  the workspace type picked for it, or a paired host by name. Scoped to the
 *  draft's host, which is where it runs and so the place shown as picked. */
export function RunTargetTab(props: Props) {
  const local = useHostId() === LOCAL_ENVIRONMENT_ID;
  return local ? <WithWorkspaceTypes {...props} /> : <RunTargetControl {...props} types={null} />;
}

/** The control for a draft of This Mac's, with its workspace types read. */
function WithWorkspaceTypes(props: Props) {
  const { chip, load } = useWorkspaceTypeChip(props.repoPath, props.provider, props.workspaceType);
  return <RunTargetControl {...props} types={chip} loadTypes={load} />;
}

function RunTargetControl({
  projectId,
  pinned,
  onPick,
  types,
  loadTypes,
}: Props & { types: WorkspaceTypeChip; loadTypes?: () => void }) {
  const hostId = useHostId();
  const environments = useAppStore((s) => s.environments);
  const { definition, check } = useProjectDefinition(projectId);
  const [open, setOpen] = useState(false);

  const places = runTargetOptions({ environments, draftHost: hostId, definition });
  const options = pinned ? places.filter((o) => o.hostId === hostId) : places;
  const picked = options.find((o) => o.hostId === hostId);
  const env = picked && environments[picked.hostId];
  if (!picked || !env) return null;

  const shown = types?.kind === "menu" ? types.shown : null;
  // With one place and no workspace type to pick for it there is nothing to
  // open: the tab only says where the workspace runs.
  if (options.length === 1 && !types) return <EnvironmentTab env={env} />;

  return (
    <span style={{ position: "relative", display: "flex", minWidth: 0 }}>
      <EnvironmentTab
        env={env}
        icon={types?.kind === "retry" ? "refresh" : undefined}
        detail={shown ? workspaceTypeLabel(shown.kind) : undefined}
        tip={picked.reason ?? types?.tip}
        menu={{
          open,
          onToggle: () => {
            if (!open) {
              check();
              loadTypes?.();
            }
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
