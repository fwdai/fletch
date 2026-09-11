import type { ProviderAuthStatus } from "@/api/types/providers";
import { Badge, type BadgeVariant } from "@/components/ui";

/** Tone and label per sign-in state. `signed_out` is a warning, not an error:
 *  the CLI is installed and one `Sign in` away from working, so it shouldn't
 *  read like something broke. */
const LABELS: Record<"signed_in" | "signed_out", { text: string; variant: BadgeVariant }> = {
  signed_in: { text: "Signed in", variant: "ok" },
  signed_out: { text: "Not signed in", variant: "warn" },
};

/** Whether a provider's CLI is logged in, as a compact pill for its row in
 *  Settings › Providers. Renders **nothing** for `unknown` and for a provider
 *  we haven't probed yet: the backend only reports `unknown` when it has no
 *  cheap check for that CLI's credential store, and a wrong "Not signed in"
 *  would send the user chasing a login they already have.
 *
 *  `detail` is the backend's short reason for a non-signed-in status; it rides
 *  on the native `title` tooltip (Badge's `hint`) so the OS positions it. It is
 *  always a fixed backend string — never a credential value. */
export function ProviderAuthBadge({
  status,
  detail,
}: {
  status?: ProviderAuthStatus;
  detail?: string | null;
}) {
  const label = status && status !== "unknown" ? LABELS[status] : null;
  if (!label) return null;
  return (
    <Badge variant={label.variant} hint={detail ?? undefined}>
      {label.text}
    </Badge>
  );
}
