import { useCallback, useEffect, useState } from "react";
import type { WorkspaceKind } from "@/api";
import { Icon } from "@/components/Icon";
import { DropdownItem, DropdownMenu, DropdownSection } from "@/components/ui/Dropdown";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore } from "@/store";
import { workspaceTypeOptionsFor, workspaceTypesFailedFor } from "@/store/workspaceTypes";
import { RailGroup, RailPiece } from "./RunRail";
import { workspaceTypeChip, workspaceTypeIcon, workspaceTypeLabel } from "./workspaceType";

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

/** The run rail's first piece: where a new agent runs, as an icon with the
 *  menu of places behind it. The piece declares it: the spawn sends the kind
 *  shown here, read from the store's one answer for this repo and provider
 *  (`store/workspaceTypes`), which is loaded on mount, when those inputs
 *  change, on every open and when a box setup event moves the default; an
 *  answer for other inputs is never shown. Nothing is drawn until the answer
 *  for these inputs lands; a failed read keeps the answer and the pick, and
 *  with no answer yet the piece offers a retry instead. */
export function WorkspaceTypePicker({ repoPath, provider, current, onPick }: Props) {
  const [open, setOpen] = useState(false);
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

  const chip = workspaceTypeChip(options, failed, current, progress);
  if (!chip) return null;

  if (chip.kind === "retry") {
    return (
      <RailGroup>
        <RailPiece glyph tip={chip.tip} onClick={load}>
          <Icon name={chip.pick ? workspaceTypeIcon(chip.pick) : "refresh"} size={14} />
        </RailPiece>
      </RailGroup>
    );
  }
  const { shown } = chip;

  return (
    <RailGroup>
      <RailPiece
        glyph
        tip={chip.tip}
        open={open}
        onClick={() => {
          if (!open) load();
          setOpen((v) => !v);
        }}
      >
        <Icon name={workspaceTypeIcon(shown.kind)} size={14} />
      </RailPiece>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <DropdownMenu className="run-menu" style={{ bottom: "calc(100% + 8px)", left: 0 }}>
            <DropdownSection>Runs in</DropdownSection>
            {chip.options.map((o) => (
              <DropdownItem
                key={o.kind}
                as="button"
                role="menuitemradio"
                aria-checked={o.kind === shown.kind}
                active={o.kind === shown.kind}
                disabled={!o.available}
                title={o.available ? undefined : (o.reason ?? undefined)}
                onClick={() => {
                  // Every click is kept, the default included: the agent
                  // lands where the piece says even if the default moves
                  // before the draft is sent.
                  onPick(o.kind);
                  setOpen(false);
                }}
              >
                <Icon name={workspaceTypeIcon(o.kind)} size={14} />
                <span className="di-l">{workspaceTypeLabel(o.kind)}</span>
                {o.default && <span className="di-m">default</span>}
                {o.kind === shown.kind && <Icon name="check" size={13} />}
              </DropdownItem>
            ))}
          </DropdownMenu>
        </>
      )}
    </RailGroup>
  );
}
