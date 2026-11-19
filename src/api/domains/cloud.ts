import { getSetting } from "@/storage/settings";
import { invokeLocal } from "../invoke";
import type { CloudProviderId, CloudProviderStatus } from "../types/cloud";

/** This Mac's engine setting for how many spare machines each connected
 *  provider keeps: one of the box settings, read and written as they are. */
const SPARES_SETTING = "box_cloud_spares";

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
   *  `account` is the provider's default (its `account_placeholder`). */
  cloudConnect: (provider: CloudProviderId, token: string, account?: string) =>
    invokeLocal<CloudProviderStatus>("cloud_connect", { provider, token, account: account ?? "" }),
  /** Forget the provider's token. Machines already created there are left as
   *  they are. */
  cloudDisconnect: (provider: CloudProviderId) =>
    invokeLocal<void>("cloud_disconnect", { provider }),
  /** How many spare machines each connected provider keeps, as stored on this
   *  Mac; `null` when never set, which is the engine's default. */
  cloudSpares: () => getSetting(SPARES_SETTING),
  /** Store how many spare machines each connected provider keeps, through the
   *  box settings' setter (`set_box_setting`), which validates it and has the
   *  engine make or destroy spares to match; `null` is back to the default.
   *  Rejects with the validation message. */
  cloudSetSpares: (count: string | null) =>
    invokeLocal<void>("set_box_setting", { key: SPARES_SETTING, value: count }),
};
