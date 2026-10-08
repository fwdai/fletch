// The webview's half of the nearby plugin (mobile/src-tauri/plugins/nearby):
// the Fletch hosts announcing themselves on this network
// (docs/remote-protocol.md, "Discovery"). Outside Tauri there is no browser;
// mock mode lists the mock host so the Pair screen's list can be worked on in
// the browser dev loop, and anything else lists nobody.

import { DEFAULT_PORT } from "@desktop/remote/types";
import { inTauri } from "@desktop/remote/ws";
import { invoke } from "@tauri-apps/api/core";
import { mockEnabled } from "./index";
import { MOCK_HOST_KEY } from "./mock";
import { hostInfo } from "./mock/fixtures";

/** One host as its announcement describes it. A claim, not a credential: the
 *  handshake is what proves `hostKey`. */
export interface NearbyHost {
  name: string;
  hostKey: string;
  port: number;
}

/** Browse for `durationMs` and answer with every host seen by the end. */
export async function browseNearby(durationMs = 2000): Promise<NearbyHost[]> {
  if (inTauri()) {
    const { hosts } = await invoke<{ hosts: NearbyHost[] }>("plugin:nearby|browse", {
      durationMs,
    });
    return hosts;
  }
  return mockEnabled() ? [{ name: hostInfo.name, hostKey: MOCK_HOST_KEY, port: DEFAULT_PORT }] : [];
}
