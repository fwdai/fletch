import { useCallback, useEffect, useState } from "react";
import { api, type WorkspaceKind, type WorkspaceTypeOption } from "@/api";
import { Icon } from "@/components/Icon";
import { Chip } from "@/components/ui/Chip";
import { DropdownItem, DropdownMenu, DropdownSection } from "@/components/ui/Dropdown";
import { Scrim } from "@/components/ui/Scrim";
import { useAppStore } from "@/store";
import { shownWorkspaceType, workspaceTypeLabel, workspaceTypeVisible } from "./workspaceType";

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

/** What the host answered: the options, or that it could not answer. */
type Answer =
  | { kind: "loading" }
  | { kind: "failed" }
  | { kind: "ok"; options: WorkspaceTypeOption[] };

/** Footer chip and menu for where a new agent runs. The options, each one's
 *  availability and reason, and the default are the host's
 *  (`available_workspace_types`); refetched when the repo, the provider or
 *  the boxes switch changes, and on every open, since an image finishing its
 *  download moves the default. Draws nothing until the host has answered, nor
 *  when no option is the default (a container engine pinned in Settings);
 *  a pick made earlier is dropped then, so the spawn never carries a choice
 *  the user cannot see. */
export function WorkspaceTypePicker({ repoPath, provider, current, onPick }: Props) {
  const [open, setOpen] = useState(false);
  const [answer, setAnswer] = useState<Answer>({ kind: "loading" });
  const boxesEnabled = useAppStore((s) => s.boxesEnabled);

  const load = useCallback(() => {
    let alive = true;
    api
      .availableWorkspaceTypes(repoPath, provider)
      .then((options) => {
        if (alive) setAnswer({ kind: "ok", options });
      })
      .catch(() => {
        if (alive) setAnswer({ kind: "failed" });
      });
    return () => {
      alive = false;
    };
  }, [repoPath, provider]);

  // `boxesEnabled` only triggers the refetch; the host reads the switch itself.
  // biome-ignore lint/correctness/useExhaustiveDependencies: refetch trigger
  useEffect(() => load(), [load, boxesEnabled]);

  const options = answer.kind === "ok" ? answer.options : null;
  const hidden = answer.kind === "failed" || (options !== null && !workspaceTypeVisible(options));
  useEffect(() => {
    if (hidden && current) onPick(undefined);
  }, [hidden, current, onPick]);

  if (!options || hidden) return null;
  const shown = shownWorkspaceType(options, current);
  if (!shown) return null;

  return (
    <span style={{ position: "relative", minWidth: 0 }}>
      <Chip
        // An option shown but unavailable (a project pinned to VMs on a
        // runtime that cannot boot one) says why the launch will fail.
        tip={shown.available ? "Where this workspace runs" : (shown.reason ?? undefined)}
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
