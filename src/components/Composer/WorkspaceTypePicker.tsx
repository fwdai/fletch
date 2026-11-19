import { useCallback, useEffect, useState } from "react";
import type { WorkspaceKind, WorkspaceTypeOption } from "@/api";
import { Icon } from "@/components/Icon";
import { DropdownItem, DropdownMenu, DropdownSection } from "@/components/ui/Dropdown";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore } from "@/store";
import { workspaceTypeOptionsFor, workspaceTypesFailedFor } from "@/store/workspaceTypes";
import {
  type WorkspaceTypeChip,
  workspaceTypeChip,
  workspaceTypeIcon,
  workspaceTypeLabel,
} from "./workspaceType";

interface Props {
  /** The repo the workspace will be created for: the project's placement and
   *  its native-app detection are read against it. */
  repoPath: string;
  /** The draft's provider: a VM runs only the providers boxes support. */
  provider: string;
  /** The user's explicit pick; `undefined` until they make one. */
  current?: WorkspaceKind;
  /** Store a pick. */
  onPick: (kind: WorkspaceKind) => void;
}

/** What the workspace-type chip draws for these inputs, from the store's one
 *  answer for this repo and provider (`store/workspaceTypes`), and `load` to
 *  read it again. Loaded on mount and when the inputs change; the answer is
 *  forgotten when the chip leaves the screen. */
export function useWorkspaceTypeChip(
  repoPath: string,
  provider: string,
  current: WorkspaceKind | undefined,
): { chip: WorkspaceTypeChip; load: () => void } {
  const options = useAppStore((s) => workspaceTypeOptionsFor(s, repoPath, provider));
  const failed = useAppStore((s) => workspaceTypesFailedFor(s, repoPath, provider));
  const progress = useAppStore((s) =>
    s.boxImage?.phase === "downloading" ? s.boxImage.progress : null,
  );
  const loadWorkspaceTypes = useAppStore((s) => s.loadWorkspaceTypes);
  const releaseWorkspaceTypes = useAppStore((s) => s.releaseWorkspaceTypes);

  const load = useCallback(
    () => void loadWorkspaceTypes(repoPath, provider),
    [loadWorkspaceTypes, repoPath, provider],
  );
  useEffect(load, [load]);
  useEffect(() => releaseWorkspaceTypes, [releaseWorkspaceTypes]);

  return { chip: workspaceTypeChip(options, failed, current, progress), load };
}

/** The menu's rows, one per workspace type: the default marked, `checked`
 *  ticked, an unavailable one disabled with the host's reason. */
export function WorkspaceTypeItems({
  options,
  checked,
  onPick,
}: {
  options: WorkspaceTypeOption[];
  checked: WorkspaceKind | null;
  onPick: (kind: WorkspaceKind) => void;
}) {
  return (
    <>
      {options.map((o) => (
        <DropdownItem
          key={o.kind}
          as="button"
          role="menuitemradio"
          aria-checked={o.kind === checked}
          active={o.kind === checked}
          disabled={!o.available}
          title={o.available ? undefined : (o.reason ?? undefined)}
          onClick={() => onPick(o.kind)}
        >
          <Icon name={workspaceTypeIcon(o.kind)} size={14} />
          <span className="di-l">{workspaceTypeLabel(o)}</span>
          {o.default && <span className="di-m">default</span>}
          {o.kind === checked && <Icon name="check" size={13} />}
        </DropdownItem>
      ))}
    </>
  );
}

/** Where a new workspace runs, a Linux VM, the macOS sandbox or a machine at
 *  a connected cloud provider: a pill in the new-workspace row beside the
 *  project and the branch. It is a property of
 *  the workspace, chosen once at its creation, so it lives with them and not
 *  in the composer. The pill declares it: the spawn sends the kind shown
 *  here, read from the store's one answer for this repo and provider
 *  (`store/workspaceTypes`), which is loaded on mount, when those inputs
 *  change, on every open and when a box setup event moves the default; an
 *  answer for other inputs is never shown. Nothing is drawn until the answer
 *  for these inputs lands; a failed read keeps the answer and the pick, and
 *  with no answer yet the pill offers a retry instead. */
export function WorkspaceTypePicker({ repoPath, provider, current, onPick }: Props) {
  const [open, setOpen] = useState(false);
  const { chip, load } = useWorkspaceTypeChip(repoPath, provider, current);
  if (!chip) return null;

  if (chip.kind === "retry") {
    return (
      <button type="button" className="pill is-action tip" data-tip={chip.tip} onClick={load}>
        <Icon name="refresh" />
        <span className="v">{chip.pick ? workspaceTypeLabel({ kind: chip.pick }) : "sandbox"}</span>
      </button>
    );
  }
  const { shown } = chip;

  return (
    <span style={{ position: "relative" }}>
      <button
        type="button"
        className="pill is-action tip"
        data-tip={chip.tip}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => {
          if (!open) load();
          setOpen((v) => !v);
        }}
      >
        <Icon name={workspaceTypeIcon(shown.kind)} />
        <span style={{ color: "var(--fg-2)" }}>in</span>
        <span className="v">{workspaceTypeLabel(shown)}</span>
        <Icon name="chevD" size={9} />
      </button>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <DropdownMenu className="ws-type-menu" style={{ bottom: "calc(100% + 6px)", left: 0 }}>
            <DropdownSection>Runs in</DropdownSection>
            <WorkspaceTypeItems
              options={chip.options}
              checked={shown.kind}
              onPick={(kind) => {
                // Every click is kept, the default included: the workspace
                // lands where the pill says even if the default moves
                // before the draft is sent.
                onPick(kind);
                setOpen(false);
              }}
            />
          </DropdownMenu>
        </>
      )}
    </span>
  );
}
