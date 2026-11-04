import type { SyntheticEvent } from "react";
import { Icon } from "@/components/Icon";
import { DropdownItem } from "@/components/ui/Dropdown";
import { MenuButton } from "@/components/ui/MenuButton";
import { useAppStore } from "@/store";
import { useGate } from "@/store/capabilities";
import { useShortcutKeys } from "@/util/shortcuts";

const stop = (e: SyntheticEvent) => e.stopPropagation();

/** The project header's `+`. Where workflows run it opens a menu — a new
 *  agent, or a workflow run — and either opens a draft of that kind. */
export function ProjectAddButton({ repoPath }: { repoPath: string }) {
  const createDraft = useAppStore((s) => s.createDraft);
  const newAgentKeys = useShortcutKeys("newAgent");
  // A host answers no `wf_*` op (docs/multi-host-plan.md §5.3, item 2), so on
  // a remote environment there is no run to offer: the + only adds an agent.
  const workflowGate = useGate("workflows");

  if (workflowGate) {
    return (
      <button
        className="padd padd-add tip"
        data-tip={`New agent  ${newAgentKeys}`}
        onClick={(e) => {
          stop(e);
          void createDraft(repoPath);
        }}
        aria-label="New agent"
      >
        <Icon name="plus" size={11} />
      </button>
    );
  }

  return (
    // The header toggles its group on click and starts a reorder on pointer
    // down. The trigger, the menu and its scrim all render inside it, so
    // neither may reach it from here.
    <span onClick={stop} onPointerDown={stop}>
      <MenuButton
        trigger={(props) => (
          <button
            className="padd padd-add tip"
            data-tip="New agent or workflow run"
            aria-label="New agent or workflow run"
            {...props}
          >
            <Icon name="plus" size={11} />
          </button>
        )}
      >
        {(close) => (
          <>
            <DropdownItem
              as="button"
              role="menuitem"
              onClick={() => {
                close();
                void createDraft(repoPath);
              }}
            >
              <span className="di-i">
                <Icon name="bot" size={13} />
              </span>
              <span className="di-l">New agent</span>
              <span className="di-m">{newAgentKeys}</span>
            </DropdownItem>
            <DropdownItem
              as="button"
              role="menuitem"
              onClick={() => {
                close();
                void createDraft(repoPath, { kind: "workflow" });
              }}
            >
              <span className="di-i">
                <Icon name="combine" size={13} />
              </span>
              <span className="di-l">Run workflow</span>
            </DropdownItem>
          </>
        )}
      </MenuButton>
    </span>
  );
}
