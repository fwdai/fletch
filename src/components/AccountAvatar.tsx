import { DEFAULT_ACCOUNT_ID } from "@/api/types/providers";
import { Icon } from "@/components/Icon";
import { accountHue } from "@/data/providerAccounts";

/** A provider account's badge: a hue-tinted rounded square with the account's
 *  initial (the terminal login, which has no name of its own, gets a terminal
 *  glyph). `spent` fills it amber, the one state worth reading at a glance. */
export function AccountAvatar({
  id,
  label,
  size = 18,
  spent = false,
}: {
  id: string;
  label: string;
  size?: number;
  spent?: boolean;
}) {
  const isDefault = id === DEFAULT_ACCOUNT_ID;
  return (
    <span
      className={`acct-avatar iflex-center ${spent ? "is-spent" : ""}`}
      style={{
        width: size,
        height: size,
        fontSize: Math.round(size * 0.56),
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
