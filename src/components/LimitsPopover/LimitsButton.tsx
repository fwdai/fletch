import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";
import { useAppStore } from "@/store";
import { useShortcutKeys } from "@/util/shortcuts";

/** The sidebar footer's way into the plan-limits popover. */
export function LimitsButton() {
  const open = useAppStore((s) => s.limitsPopoverOpen);
  const toggle = useAppStore((s) => s.toggleLimitsPopover);
  const keys = useShortcutKeys("limits");
  return (
    <IconButton
      tip={keys ? `Plan limits (${keys})` : "Plan limits"}
      aria-label="Plan limits"
      aria-expanded={open}
      active={open}
      onClick={() => toggle()}
    >
      <Icon name="gauge" />
    </IconButton>
  );
}
