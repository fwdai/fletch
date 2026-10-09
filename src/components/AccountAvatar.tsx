import { DEFAULT_ACCOUNT_ID } from "@/api/types/providers";
import { Icon } from "@/components/Icon";
import { accountHue } from "@/data/providerAccounts";

/** A provider account's avatar: a round, hue-tinted badge with the account's
 *  initial (the terminal login, which has no name of its own, gets a terminal
 *  glyph). Round where a custom agent's monogram is square, so the two never
 *  read as each other. `ring` outlines it in the surface colour for sitting
 *  over another icon; `spent` turns the outline amber. */
export function AccountAvatar({
  id,
  label,
  size = 16,
  ring = false,
  spent = false,
}: {
  id: string;
  label: string;
  size?: number;
  ring?: boolean;
  spent?: boolean;
}) {
  const isDefault = id === DEFAULT_ACCOUNT_ID;
  return (
    <span
      className={`acct-avatar iflex-center ${ring ? "has-ring" : ""} ${spent ? "is-spent" : ""}`}
      style={{
        width: size,
        height: size,
        fontSize: Math.round(size * 0.58),
        ["--h" as string]: isDefault ? 250 : accountHue(id),
      }}
      aria-hidden
    >
      {isDefault ? (
        <Icon name="terminal" size={Math.round(size * 0.62)} />
      ) : (
        (label.trim()[0] ?? "·").toUpperCase()
      )}
    </span>
  );
}
