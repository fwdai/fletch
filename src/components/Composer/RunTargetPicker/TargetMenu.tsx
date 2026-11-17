import { Fragment } from "react";
import type { WorkspaceKind } from "@/api";
import { DropdownMenu, DropdownSection } from "@/components/ui/Dropdown";
import type { EnvironmentEntry, EnvironmentId } from "@/store/environments";
import { WorkspaceTypeItems } from "../WorkspaceTypePicker";
import type { WorkspaceTypeChip } from "../workspaceType";
import { RUN_TARGET_SECTIONS, type RunTargetOption } from "./options";
import { TargetItem } from "./TargetItem";

interface Props {
  options: RunTargetOption[];
  environments: Record<EnvironmentId, EnvironmentEntry>;
  /** The place picked now. */
  picked: RunTargetOption;
  /** This Mac's workspace types, listed in its place once they are known,
   *  for a draft of This Mac's. */
  types: WorkspaceTypeChip;
  /** A place, and for This Mac the workspace type clicked with it. */
  onPick: (hostId: EnvironmentId, workspaceType?: WorkspaceKind) => void;
}

/** The menu of places, a section per kind in the order the options come. */
export function TargetMenu({ options, environments, picked, types, onPick }: Props) {
  const kinds = [...new Set(options.map((o) => o.kind))];
  return (
    <DropdownMenu className="ws-type-menu" style={{ bottom: "calc(100% + 6px)", left: 0 }}>
      {kinds.map((kind) => (
        <Fragment key={kind}>
          <DropdownSection>{RUN_TARGET_SECTIONS[kind]}</DropdownSection>
          {options
            .filter((o) => o.kind === kind)
            .map((o) =>
              o.kind === "local" && types?.kind === "menu" ? (
                // Every click is kept, the default included, as on the
                // workspace-type pill.
                <WorkspaceTypeItems
                  key={o.id}
                  options={types.options}
                  checked={o.id === picked.id ? types.shown.kind : null}
                  onPick={(type) => onPick(o.hostId, type)}
                />
              ) : (
                <TargetItem
                  key={o.id}
                  option={o}
                  env={environments[o.hostId]}
                  checked={o.id === picked.id}
                  onPick={() => onPick(o.hostId)}
                />
              ),
            )}
        </Fragment>
      ))}
    </DropdownMenu>
  );
}
