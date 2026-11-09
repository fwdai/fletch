import type { RemoteStatus } from "@/api";

/** What stops a paired device reaching this Mac, in one line, or null when
 *  nothing does: a healthy connection says nothing beyond its switches. The
 *  listener first, since without it nothing reaches this Mac at all; then the
 *  relay, which is how devices off this network do and which has no switch
 *  of its own, so this line is the only place its trouble shows. */
export function connectionProblem(status: RemoteStatus | null): string | null {
  if (!status?.enabled) return null;
  if (!status.listening) return "On, but the port could not be opened.";
  if (status.addresses.length === 0) {
    return "No network address found. Connect this Mac to Wi-Fi or Ethernet.";
  }
  if (status.relay.state === "error") {
    const why = status.relay.error ? `: ${status.relay.error}` : ".";
    return `Devices off this network can't reach this Mac right now${why}`;
  }
  return null;
}
