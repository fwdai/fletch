import { useCallback, useEffect, useState } from "react";
import { api, type WorkspaceKind, type WorkspaceTypeOption } from "@/api";
import { Icon } from "@/components/Icon";
import { Chip } from "@/components/ui/Chip";
import { DropdownItem, DropdownMenu, DropdownSection } from "@/components/ui/Dropdown";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore } from "@/store";
import { shownWorkspaceType, workspaceTypeLabel, workspaceTypeOverride } from "./workspaceType";

/** The composer's workspace type for a new agent draft. Existing sessions keep
 *  the engine stamped at their creation and get no control. */
export interface WorkspaceTypeControl {
  /** The repo the agent will be started in: the project's placement and its
   *  native-app detection are read against it. */
  repoPath: string;
  /** The user's pick, kept only while it differs from the host's default. */
  current?: WorkspaceKind;
  /** Store the pick: `undefined` when it is the default (or no longer
   *  offered), so the spawn carries nothing and the host decides. */
  onPick: (kind: WorkspaceKind | undefined) => void;
}

interface Props extends WorkspaceTypeControl {
  /** The draft's provider: a VM runs only the providers boxes support. */
  provider: string;
}

const icon = (kind: WorkspaceKind | null) =>
  kind === "box" ? "cube" : kind === "host" ? "folderLock" : "settings";

/** Footer chip and menu for where a new agent runs. The options, each one's
 *  availability and reason, and the default are the host's
 *  (`available_workspace_types`); refetched when the repo, the provider or
 *  the boxes switch changes, and on every open, since an image finishing its
 *  download moves the default. Draws nothing until the host has answered. */
export function WorkspaceTypePicker({ repoPath, provider, current, onPick }: Props) {
  const [open, setOpen] = useState(false);
  const [options, setOptions] = useState<WorkspaceTypeOption[] | null>(null);
  const boxesEnabled = useAppStore((s) => s.boxesEnabled);

  const load = useCallback(() => {
    let alive = true;
    api
      .availableWorkspaceTypes(repoPath, provider)
      .then((list) => {
        if (alive) setOptions(list);
      })
      .catch(() => {
        if (alive) setOptions(null);
      });
    return () => {
      alive = false;
    };
  }, [repoPath, provider]);

  // `boxesEnabled` only triggers the refetch; the host reads the switch itself.
  // biome-ignore lint/correctness/useExhaustiveDependencies: refetch trigger
  useEffect(() => load(), [load, boxesEnabled]);

  // A pick the host no longer offers, or that became the default, is dropped
  // so the spawn never carries a stale choice.
  useEffect(() => {
    if (!options) return;
    const next = workspaceTypeOverride(options, current);
    if (next !== current) onPick(next);
  }, [options, current, onPick]);

  if (!options) return null;
  const shown = shownWorkspaceType(options, current);

  return (
    <span style={{ position: "relative", minWidth: 0 }}>
      <Chip
        tip="Where this workspace runs"
        onClick={() => {
          if (!open) load();
          setOpen((v) => !v);
        }}
      >
        <Icon name={icon(shown)} size={11} />
        <span>{shown ? workspaceTypeLabel(shown) : "As in Settings"}</span>
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
                active={o.kind === shown}
                disabled={!o.available}
                style={{ padding: "7px 9px" }}
                title={o.reason ?? undefined}
                onClick={() => {
                  onPick(workspaceTypeOverride(options, o.kind));
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
