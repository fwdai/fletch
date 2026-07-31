import { Icon } from "@/components/Icon";

interface Props {
  value: boolean;
  onChange: (value: boolean) => void;
}

/** Share-with-sandbox control, styled as a "drop it in the box" toggle rather
 *  than a generic pill: the cube *is* the sandbox — a muted outline when the
 *  variable is withheld, lit in the accent with a soft glow when it's shared
 *  in. (Inspired by Cloudflare's proxy-cloud toggle.) Carries its own tooltip
 *  since the icon alone doesn't spell out what it does. */
export function SandboxToggle({ value, onChange }: Props) {
  return (
    <button
      type="button"
      className="ev-sbx tip"
      data-on={value ? "1" : "0"}
      data-tip={value ? "Shared with sandbox" : "Not shared with sandbox"}
      aria-pressed={value}
      aria-label="Share with the sandbox"
      onClick={() => onChange(!value)}
    >
      <Icon name="cube" size={15} />
    </button>
  );
}
