// AgentAvatar — the one avatar for a workflow step's agent, used everywhere
// (step card, list preview, picker). A base provider shows its brand icon via
// the shared ProviderIcon; a custom agent shows its colored initials in the
// same `.chip-mono` shell, so the two read as one family at any size.

import type { CSSProperties } from "react";
import { ProviderIcon } from "../../components/ProviderIcon";

export function AgentAvatar({
  custom,
  slug,
  short,
  hue,
  size = 22,
}: {
  custom: boolean;
  /** Provider slug for the brand icon (base agents). */
  slug: string;
  short: string;
  hue: number;
  size?: number;
}) {
  // Base provider → brand icon (with its own initials fallback).
  if (!custom) {
    return <ProviderIcon slug={slug} short={short} hue={hue} size={size} />;
  }
  // Custom agent → initials in the identical chip shell. The sizing mirrors
  // ProviderIcon so a 22px custom chip matches a 22px brand chip exactly.
  const style: CSSProperties = {
    width: size,
    height: size,
    borderRadius: Math.max(3, Math.round(size * 0.233)),
    fontSize: Math.round(size * 0.35 * 10) / 10,
    ["--ph-h" as string]: hue,
    ["--ph" as string]: "oklch(.65 .13 var(--ph-h))",
  };
  return (
    <span className="chip-mono iflex-center" style={style}>
      {short}
    </span>
  );
}
