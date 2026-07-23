import { Icon, type IconName } from "@/components/Icon";

export interface ActionCardProps {
  icon: IconName;
  title: string;
  sub: string;
  onClick: () => void;
  /** "primary" is the single hero action — larger, accent-tinted. */
  tone?: "primary" | "default";
  /** Optional keyboard hint shown on the trailing edge (e.g. "⌘N"). */
  kbd?: string;
  /** While true the icon spins and the card is non-interactive. */
  busy?: boolean;
  disabled?: boolean;
}

/** One quick action on the Home screen. A big, single-purpose button: an
 *  accent icon tile, a title + one line of context, and a trailing affordance
 *  (a keyboard hint, or an arrow that nudges on hover). */
export function ActionCard({
  icon,
  title,
  sub,
  onClick,
  tone = "default",
  kbd,
  busy = false,
  disabled = false,
}: ActionCardProps) {
  const primary = tone === "primary";
  return (
    <button
      type="button"
      className={`home-card${primary ? " primary" : ""}`}
      onClick={onClick}
      disabled={disabled || busy}
    >
      <span className={`home-card-icon flex-center${busy ? " spin" : ""}`}>
        <Icon name={busy ? "refresh" : icon} size={primary ? 19 : 16} />
      </span>
      <span className="home-card-text">
        <span className="home-card-title">{title}</span>
        <span className="home-card-sub truncate">{sub}</span>
      </span>
      {kbd ? (
        <kbd className="home-kbd">{kbd}</kbd>
      ) : (
        <Icon name="arrowR" size={15} className="home-card-arrow" />
      )}
    </button>
  );
}
