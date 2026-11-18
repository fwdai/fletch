import { invokeLocal } from "../invoke";
import type { CloudProviderId, CloudProviderStatus } from "../types/cloud";

/** The cloud providers whose machines a workspace can run on. A provider is an
 *  account on THIS Mac, like Linear: its token lives in this machine's keychain
 *  and no `cloud_*` op is on a host's table, so routing them would answer
 *  `UNKNOWN_OP` the moment a host is the active environment. */
export const cloudApi = {
  /** One entry per provider the backend knows, connected or not. Reads the
   *  keychain only; nothing goes over the network. */
  cloudProviders: () => invokeLocal<CloudProviderStatus[]>("cloud_providers"),
  /** Check `token` with the provider, then keep it. Rejects, in words fit to
   *  show, when the provider refuses it, and nothing is stored then. An empty
   *  `org` is the provider's default (`personal` for Fly). */
  cloudConnect: (provider: CloudProviderId, token: string, org?: string) =>
    invokeLocal<CloudProviderStatus>("cloud_connect", { provider, token, org: org ?? "" }),
  /** Forget the provider's token. Machines already created there are left as
   *  they are. */
  cloudDisconnect: (provider: CloudProviderId) =>
    invokeLocal<void>("cloud_disconnect", { provider }),
};
