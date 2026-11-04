import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";

/** The workspace's VM network for this turn, beside the folders: a globe that
 *  is lit while any host passes, and opens or closes it. Under the Ask me and
 *  Allow list postures a VM reaches only the allow-list; opening it for the
 *  turn in progress (or the next one) lets the agent reach anything until the
 *  turn ends, when the posture resumes on its own. Nothing shows under Open,
 *  where there is nothing to open. */
export function NetworkTurn({
  open,
  onToggle,
}: {
  open: boolean;
  onToggle: (open: boolean) => void;
}) {
  const tip = open
    ? "The network is open until this turn ends. Click to close it."
    : "Open the network for this turn: any host, until the turn ends";
  return (
    <IconButton
      className="composer-action"
      tip={tip}
      aria-label={open ? "Close the network" : "Open the network for this turn"}
      active={open}
      onClick={() => onToggle(!open)}
    >
      <Icon name="globe" size={15} />
    </IconButton>
  );
}
