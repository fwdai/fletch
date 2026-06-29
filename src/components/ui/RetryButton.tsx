import { Icon } from "../Icon";
import { IconButton } from "./IconButton";

/** Re-runs a failed chat turn. Sits beside CopyButton in a message's
 *  hover-revealed action row, sharing its xs icon-button styling. Stateless —
 *  the parent owns the retry action. */
export function RetryButton({
  onClick,
  tip = "Retry",
  className,
}: {
  onClick: () => void;
  tip?: string;
  className?: string;
}) {
  return (
    <IconButton
      size="xs"
      tip={tip}
      className={className}
      onClick={onClick}
      aria-label="Retry message"
    >
      <Icon name="refresh" />
    </IconButton>
  );
}
