import { Fragment } from "react";
import { DropdownMenu, DropdownSection } from "@/components/ui/Dropdown";
import type { EnvironmentEntry, EnvironmentId } from "@/store/environments";
import { RUN_TARGET_SECTIONS, type RunTargetOption } from "./options";
import { TargetItem } from "./TargetItem";

interface Props {
  options: RunTargetOption[];
  environments: Record<EnvironmentId, EnvironmentEntry>;
  /** The place picked now. */
  picked: RunTargetOption;
  onPick: (hostId: EnvironmentId) => void;
}

/** The menu of places, a section per kind in the order the options come.
 *  A kind with a heading has one; This Mac is one place and needs none. */
export function TargetMenu({ options, environments, picked, onPick }: Props) {
  const kinds = [...new Set(options.map((o) => o.kind))];
  return (
    <DropdownMenu className="ws-type-menu" style={{ bottom: "calc(100% + 6px)", left: 0 }}>
      {kinds.map((kind) => (
        <Fragment key={kind}>
          {RUN_TARGET_SECTIONS[kind] && (
            <DropdownSection>{RUN_TARGET_SECTIONS[kind]}</DropdownSection>
          )}
          {options
            .filter((o) => o.kind === kind)
            .map((o) => (
              <TargetItem
                key={o.id}
                option={o}
                env={environments[o.hostId]}
                checked={o.id === picked.id}
                onPick={() => onPick(o.hostId)}
              />
            ))}
        </Fragment>
      ))}
    </DropdownMenu>
  );
}
