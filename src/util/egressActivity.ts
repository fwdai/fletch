import type { HostActivity } from "@/api";

/** The `via` of an allowed connection, as `egress_log` spells it: on the
 *  allow-list, admitted by the Open posture, admitted by a turn's open
 *  network. */
export const ALLOWED_VIAS = ["listed", "open", "turn"] as const;

/** The `via` of a refused connection: the host, its port, or known
 *  telemetry refused quietly. */
export const REFUSED_VIAS = ["host", "port", "quiet"] as const;

/** Where a host sits in the recent lists. */
export type HostStanding = "reached" | "blocked";

/** What the host's newest connection says: reached when it was admitted,
 *  blocked when it was refused. A host with both in its window sits where its
 *  last connection put it, and the counts say the rest. Never by the counts
 *  alone: a host allowed under Open and refused since the posture tightened
 *  is blocked now, however many times it got through before. A `via` this
 *  client does not know is read as refused, the side that asks for a look. */
export function hostStanding(h: Pick<HostActivity, "last_via">): HostStanding {
  return (ALLOWED_VIAS as readonly string[]).includes(h.last_via) ? "reached" : "blocked";
}

/** How the newest connection was answered, for a row; every value spelled
 *  out, an unknown one shown as it came. */
export function viaLabel(via: string): string {
  switch (via) {
    case "listed":
      return "on a list";
    case "open":
      return "open network";
    case "turn":
      return "opened for a turn";
    case "host":
      return "refused";
    case "port":
      return "port not allowed";
    case "quiet":
      return "telemetry, refused quietly";
    default:
      return via;
  }
}

/** The host's connections in the window, both sides when it has both:
 *  "12 connections", "3 refused", "12 connections, 3 refused". Led by the
 *  side the host stands on. */
export function countsLabel(h: Pick<HostActivity, "allowed" | "refused" | "last_via">): string {
  const allowed = h.allowed > 0 ? plural(h.allowed, "connection") : null;
  const refused = h.refused > 0 ? `${h.refused} refused` : null;
  const parts =
    hostStanding(h) === "reached" ? [allowed, refused] : [refused, allowed && `${allowed} before`];
  return parts.filter((p): p is string => !!p).join(", ");
}

/** Whether a list could change the host's standing: not a port refusal (no
 *  list allows a port), not a host already on a list. */
export function canAllow(h: Pick<HostActivity, "last_via">): boolean {
  return h.last_via !== "port" && h.last_via !== "listed";
}

function plural(n: number, noun: string): string {
  return n === 1 ? `1 ${noun}` : `${n} ${noun}s`;
}
