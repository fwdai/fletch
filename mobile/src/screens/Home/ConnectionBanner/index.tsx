import { severalHosts, useApp } from "../../../store";
import { HostBanner } from "./HostBanner";
import { HostsBanner } from "./HostsBanner";

/** The slim status line over a cached project list while a link to a paired
 *  Mac is down or coming back. Persistent by nature — it is the connection
 *  state, not a message about it — so there is no dismiss; it goes when the
 *  links are up.
 *
 *  Home shows it only over a list. With nothing cached the whole body is the
 *  connection state instead (`HostState`). */
export function ConnectionBanner() {
  return useApp(severalHosts) ? <HostsBanner /> : <HostBanner />;
}
