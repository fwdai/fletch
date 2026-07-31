import { Icon, type IconName } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import type { AppState } from "@/store";
import { useAppStore } from "@/store";

type Side = "left" | "right";

interface SideSpec {
  icon: IconName;
  /** What the tooltip calls this rail — "Show sidebar" / "Hide panel". */
  noun: string;
  kbd: string;
  collapsed: (s: AppState) => boolean;
  toggle: (s: AppState) => () => void;
}

/** Everything that differs between the two rails. Keeping it as a table (rather
 *  than branching in the body) is what makes divergence impossible: adding a
 *  tooltip, an icon, or a state class means editing one row, and the other side
 *  either gets it too or visibly doesn't. */
const SIDES: Record<Side, SideSpec> = {
  left: {
    icon: "sidebarL",
    noun: "sidebar",
    kbd: "⌘B",
    collapsed: (s) => s.leftCollapsed,
    toggle: (s) => s.toggleLeft,
  },
  right: {
    icon: "sidebarR",
    noun: "panel",
    kbd: "⌘/",
    collapsed: (s) => s.rightCollapsed,
    toggle: (s) => s.toggleRight,
  },
};

/** Show/hide toggle for one of the two side rails. Shared by every pane that
 *  renders a `.center-h` header (Workspace, Home, Mission Control, the draft
 *  empty state, and the workflow run monitor) so the icon, the tooltip wording,
 *  and the resting appearance can't drift between call sites — they had: the
 *  right toggle carried `active`, and two RunView copies said "Toggle sidebar"
 *  instead of the show/hide pair.
 *
 *  Deliberately never `active`. Both rails are open by default, so an accent
 *  tied to "open" would be the resting state — permanently orange, carrying no
 *  information, and competing with the accents that do mean something (status
 *  dots, the git CTA). The glyph says which rail and the tooltip says what a
 *  click will do; that's the whole signal. Reserve `IconButton`'s `active` for
 *  states that are off by default, the way the composer's pickers use it. */
export function PanelToggle({ side }: { side: Side }) {
  const { icon, noun, kbd } = SIDES[side];
  const collapsed = useAppStore(SIDES[side].collapsed);
  const toggle = useAppStore(SIDES[side].toggle);
  return (
    <IconButton tip={`${collapsed ? "Show" : "Hide"} ${noun} (${kbd})`} onClick={toggle}>
      <Icon name={icon} />
    </IconButton>
  );
}
