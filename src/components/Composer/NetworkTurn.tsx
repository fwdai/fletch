import { Icon } from "@/components/Icon";
import { IconButton } from "@/components/ui/IconButton";

/** The sign that the workspace's VM network is open for this turn: a lit globe
 *  in the composer's footer, there only while it is, and a click closes it.
 *  Under the Ask me and Allow list postures a VM reaches only the allow-list;
 *  opening the network (from the add menu) lets the agent reach anything until
 *  the turn ends, when the posture resumes on its own. */
export function NetworkTurn({ onClose }: { onClose: () => void }) {
  return (
    <IconButton
      className="composer-action"
      tip="The network is open until this turn ends. Click to close it."
      aria-label="Close the network"
      active
      onClick={onClose}
    >
      <Icon name="globe" size={15} />
    </IconButton>
  );
}
