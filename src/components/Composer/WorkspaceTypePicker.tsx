import { useCallback, useEffect, useState } from "react";
import type { WorkspaceKind } from "@/api";
import { Icon } from "@/components/Icon";
import { Chip } from "@/components/ui/Chip";
import { DropdownItem, DropdownMenu, DropdownSection } from "@/components/ui/Dropdown";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore } from "@/store";
import { workspaceTypeOptionsFor, workspaceTypesFailedFor } from "@/store/workspaceTypes";
import { workspaceTypeChip, workspaceTypeLabel, workspaceTypePickDropped } from "./workspaceType";

/** The composer's workspace type for a new agent draft. Existing sessions keep
 *  the engine stamped at their creation and get no control. */
export interface WorkspaceTypeControl {
  /** The repo the agent will be started in: the project's placement and its
   *  native-app detection are read against it. */
  repoPath: string;
  /** The user's explicit pick; `undefined` until they make one. */
  current?: WorkspaceKind;
  /** Store a pick, or `undefined` to leave it to the host again. */
  onPick: (kind: WorkspaceKind | undefined) => void;
}

interface Props extends WorkspaceTypeControl {
  /** The draft's provider: a VM runs only the providers boxes support. */
  provider: string;
}

const icon = (kind: WorkspaceKind) => (kind === "box" ? "cube" : "folderLock");

/** Footer chip and menu for where a new agent runs. The chip declares it: the
 *  spawn sends the kind shown here, read from the store's one answer for this
 *  repo, provider and settings (`store/workspaceTypes`), which is loaded on
 *  mount, when those inputs change, on every open and when a box setup event
 *  moves the default; an answer for other inputs is never shown.
 *  Nothing is drawn until the answer for these inputs lands, nor when the host
 *  answers that no option is the default (a container engine pinned in
 *  Settings), which also drops the pick; a failed read keeps the answer and
 *  the pick, and with no answer yet the chip offers a retry instead. */
export function WorkspaceTypePicker({ repoPath, provider, current, onPick }: Props) {
  const [open, setOpen] = useState(false);
  const options = useAppStore((s) => workspaceTypeOptionsFor(s, repoPath, provider));
  const failed = useAppStore((s) => workspaceTypesFailedFor(s, repoPath, provider));
  const loadWorkspaceTypes = useAppStore((s) => s.loadWorkspaceTypes);
  const releaseWorkspaceTypes = useAppStore((s) => s.releaseWorkspaceTypes);
  // Part of the answer's key: a change drops the answer, so read again.
  const sandboxEngine = useAppStore((s) => s.sandboxEngine);
  const boxesEnabled = useAppStore((s) => s.boxesEnabled);

  const load = useCallback(
    () => void loadWorkspaceTypes(repoPath, provider),
    [loadWorkspaceTypes, repoPath, provider],
  );
  // biome-ignore lint/correctness/useExhaustiveDependencies: the settings are refetch triggers
  useEffect(load, [load, sandboxEngine, boxesEnabled]);
  useEffect(() => releaseWorkspaceTypes, [releaseWorkspaceTypes]);

  const dropPick = workspaceTypePickDropped(options);
  useEffect(() => {
    if (dropPick && current) onPick(undefined);
  }, [dropPick, current, onPick]);

  const chip = workspaceTypeChip(options, failed, current);
  if (!chip) return null;

  if (chip.kind === "retry") {
    return (
      <span style={{ position: "relative", minWidth: 0 }}>
        <Chip tip={chip.tip} onClick={load}>
          <Icon name={chip.pick ? icon(chip.pick) : "refresh"} size={11} />
          <span>{chip.label}</span>
        </Chip>
      </span>
    );
  }
  const { shown } = chip;

  return (
    <span style={{ position: "relative", minWidth: 0 }}>
      <Chip
        tip={chip.tip}
        onClick={() => {
          if (!open) load();
          setOpen((v) => !v);
        }}
      >
        <Icon name={icon(shown.kind)} size={11} />
        <span>{workspaceTypeLabel(shown.kind)}</span>
      </Chip>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <DropdownMenu
            style={{ bottom: "calc(100% + 6px)", left: 0, padding: 0, overflow: "hidden" }}
          >
            <DropdownSection>Runs in</DropdownSection>
            {chip.options.map((o) => (
              <DropdownItem
                key={o.kind}
                active={o.kind === shown.kind}
                disabled={!o.available}
                style={{ padding: "7px 9px" }}
                title={o.reason ?? undefined}
                onClick={() => {
                  // Every click is kept, the default included: the agent
                  // lands where the chip says even if the default moves
                  // before the draft is sent.
                  onPick(o.kind);
                  setOpen(false);
                }}
              >
                <Icon name={icon(o.kind)} size={14} />
                <span className="di-l">{workspaceTypeLabel(o.kind)}</span>
                {o.default && <span className="di-m">default</span>}
              </DropdownItem>
            ))}
          </DropdownMenu>
        </>
      )}
    </span>
  );
}
