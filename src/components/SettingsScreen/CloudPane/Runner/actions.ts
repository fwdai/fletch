// The two runner actions that change which hosts this Mac is paired with, so
// they are the store's action and the host registry's together: creating the
// runner pairs this Mac with it, deleting it forgets that pairing. Plain
// functions over this Mac's store, so they finish whether or not the pane is
// still on screen.

import type { CloudProviderId, CloudRunnerDeleted } from "@/api";
import { addRelayHost, removeHost } from "@/remote/hosts";
import { useAppStore } from "@/store";

/** Create the runner at `provider`, then keep it as a paired host and dial
 *  it, so it is a place to run a workspace at once: the engine authorised this
 *  Mac's key on it while setting it up, so no pairing code is needed. */
export async function createRunner(
  provider: CloudProviderId,
  claudeToken: string,
  name: string,
): Promise<void> {
  const record = await useAppStore.getState().createCloudRunner(provider, claudeToken, name);
  await addRelayHost({ hostKey: record.hostId, name: record.name, relay: record.relay });
}

/** Delete the runner, then forget the paired host it was (`hostKey`, its
 *  record's `hostId`; none for a set-up that never finished), so nothing
 *  dials a runner that is gone. */
export async function deleteRunner(
  force: boolean,
  hostKey: string | undefined,
): Promise<CloudRunnerDeleted> {
  const deleted = await useAppStore.getState().deleteCloudRunner(force);
  if (hostKey) await removeHost(hostKey);
  return deleted;
}
