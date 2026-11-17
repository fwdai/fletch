import type { ConnectionState } from "./types";

/** How a paired host reads at a glance, in every app:
 *
 *  - `connected`: live.
 *  - `connecting`: a dial or a pairing is in flight.
 *  - `inactive`: not reachable now, which is not a fault. Never dialled, or
 *    down with the client retrying on its own: a Mac that is asleep or off the
 *    network.
 *  - `problem`: stopped on something only the user can clear, such as a
 *    pairing the host no longer honours. The client schedules no retry
 *    behind those. */
export type HostTone = "connected" | "connecting" | "inactive" | "problem";

/** The tone of a host whose client is in `connection`, with a retry scheduled
 *  or not (`RemoteClient.retrying`). */
export function hostTone(connection: ConnectionState, retrying: boolean | undefined): HostTone {
  switch (connection) {
    case "connected":
      return "connected";
    case "connecting":
    case "pairing":
      return "connecting";
    case "error":
      return retrying ? "inactive" : "problem";
    default:
      return "inactive";
  }
}
