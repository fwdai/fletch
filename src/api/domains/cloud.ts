import { invokeLocal } from "../invoke";
import type {
  CloudProviderId,
  CloudProviderStatus,
  CloudRunnerDeleted,
  CloudRunnerInputs,
  CloudRunnerPairing,
  CloudRunnerRecord,
  CloudRunnerStatus,
} from "../types/cloud";

/** The cloud providers whose machines a workspace can run on, and this
 *  install's runner at one. A provider is an account on THIS Mac, like Linear:
 *  its token lives in this machine's keychain and no `cloud_*` op is on a
 *  host's table, so routing them would answer `UNKNOWN_OP` the moment a host
 *  is the active environment. */
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

  /** Where the runner is. Asks its provider when there is one, so it is read
   *  when Settings › Cloud opens and after each change, not in the
   *  background. */
  cloudRunnerStatus: () => invokeLocal<CloudRunnerStatus>("cloud_runner_status"),
  /** Create the runner at `provider` and set it up; resolves with its record
   *  once it is, which takes minutes and waits for its GitHub code to be
   *  entered. Each step arrives as `cloud-runner:progress`. A failure has
   *  already undone what was made. */
  cloudRunnerCreate: (provider: CloudProviderId, inputs: CloudRunnerInputs) =>
    invokeLocal<CloudRunnerRecord>("cloud_runner_create", { provider, inputs }),
  /** Start the runner's machine; resolves once it runs. */
  cloudRunnerStart: () => invokeLocal<void>("cloud_runner_start"),
  /** Stop the runner's machine; resolves once it has stopped. */
  cloudRunnerStop: () => invokeLocal<void>("cloud_runner_stop"),
  /** Delete the runner and its workspaces' machines. Without `force`, a
   *  runner that cannot be reached is refused, in words fit to show. */
  cloudRunnerDelete: (force: boolean) =>
    invokeLocal<CloudRunnerDeleted>("cloud_runner_delete", { force }),
  /** A pairing code for another device, from the running runner. A null
   *  `preset` is full access. */
  cloudRunnerPairingLink: () =>
    invokeLocal<CloudRunnerPairing>("cloud_runner_pairing_link", { preset: null }),
};
