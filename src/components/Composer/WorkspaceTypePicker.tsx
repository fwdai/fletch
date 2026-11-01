import { useCallback, useEffect, useRef, useState } from "react";
import { api, type WorkspaceKind, type WorkspaceTypeOption } from "@/api";
import { Icon } from "@/components/Icon";
import { Chip } from "@/components/ui/Chip";
import { DropdownItem, DropdownMenu, DropdownSection } from "@/components/ui/Dropdown";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore } from "@/store";
import {
  answerWith,
  UNANSWERED,
  type WorkspaceTypeAnswer,
  workspaceTypeChip,
  workspaceTypeLabel,
  workspaceTypePickDropped,
  workspaceTypeResponseLands,
} from "./workspaceType";

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

/** Footer chip and menu for where a new agent runs. The options, each one's
 *  availability and reason, and the default are the host's
 *  (`available_workspace_types`); every read (on mount, when the repo, the
 *  provider or the boxes switch changes, and on every open, since an image
 *  finishing its download moves the default) goes through one numbered `load`,
 *  and only the latest read for the current repo and provider lands. A failed
 *  read keeps the options already shown and the user's pick; before any read
 *  has succeeded the chip draws nothing while loading and offers a retry once
 *  one failed. The pick is dropped only when the host answers that no option
 *  is the default (a container engine pinned in Settings), and the chip then
 *  stays away. */
export function WorkspaceTypePicker({ repoPath, provider, current, onPick }: Props) {
  const [open, setOpen] = useState(false);
  const [answer, setAnswer] = useState<WorkspaceTypeAnswer>(UNANSWERED);
  const boxesEnabled = useAppStore((s) => s.boxesEnabled);
  const latestId = useRef(0);
  const inputs = useRef({ repoPath, provider });
  inputs.current = { repoPath, provider };

  const load = useCallback(() => {
    latestId.current += 1;
    const request = { id: latestId.current, repoPath, provider };
    const land = (read: WorkspaceTypeOption[] | "failed") => {
      if (workspaceTypeResponseLands(request, latestId.current, inputs.current)) {
        setAnswer((prev) => answerWith(prev, read));
      }
    };
    api.availableWorkspaceTypes(repoPath, provider).then(land, () => land("failed"));
  }, [repoPath, provider]);

  // `boxesEnabled` only triggers the refetch; the host reads the switch itself.
  // Unmounting, or the inputs changing, invalidates the read in flight.
  // biome-ignore lint/correctness/useExhaustiveDependencies: refetch trigger
  useEffect(() => {
    load();
    return () => {
      latestId.current += 1;
    };
  }, [load, boxesEnabled]);

  const dropPick = workspaceTypePickDropped(answer);
  useEffect(() => {
    if (dropPick && current) onPick(undefined);
  }, [dropPick, current, onPick]);

  const chip = workspaceTypeChip(answer, current);
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
  const { options, shown } = chip;

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
            {options.map((o) => (
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
