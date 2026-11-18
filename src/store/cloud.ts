import type { CloudProviderId, CloudProviderStatus } from "@/api";
import type { SliceCreator } from "./types";

export interface CloudSlice {
  /** The cloud providers on this Mac, one per provider the backend knows, and
   *  whether each is connected. `null` until the first read. Always this
   *  Mac's, like `linear`: the tokens are in this machine's keychain and no
   *  `cloud_*` op is on the wire, so it is not a host key — whatever offers a
   *  workspace in the cloud reads the answer Settings › Cloud writes. */
  cloudProviders: CloudProviderStatus[] | null;

  /** Re-read `cloudProviders`. Rejects with the reason, keeping the last list. */
  refreshCloudProviders: () => Promise<void>;
  /** Check `token` with the provider and keep it; the provider's entry takes
   *  the answer. Rejects with the provider's reason, the list unchanged. The
   *  token goes to the backend and is held nowhere here. */
  connectCloud: (provider: CloudProviderId, token: string, org: string) => Promise<void>;
  /** Forget the provider's token. Its machines stay where they are. */
  disconnectCloud: (provider: CloudProviderId) => Promise<void>;
}

/** `list` with `status` in its provider's place, or at the end for a provider
 *  it did not have. */
export function withCloudProvider(
  list: CloudProviderStatus[] | null,
  status: CloudProviderStatus,
): CloudProviderStatus[] {
  const current = list ?? [];
  return current.some((p) => p.id === status.id)
    ? current.map((p) => (p.id === status.id ? status : p))
    : [...current, status];
}

export const createCloudSlice: SliceCreator<CloudSlice> = (set, _get, { api }) => ({
  cloudProviders: null,

  refreshCloudProviders: async () => {
    set({ cloudProviders: await api.cloudProviders() });
  },

  connectCloud: async (provider, token, org) => {
    const status = await api.cloudConnect(provider, token, org);
    set((s) => ({ cloudProviders: withCloudProvider(s.cloudProviders, status) }));
  },

  // `cloud_disconnect` answers nothing, and what it leaves is known: the same
  // provider with no token, so no account.
  disconnectCloud: async (provider) => {
    await api.cloudDisconnect(provider);
    set((s) => ({
      cloudProviders:
        s.cloudProviders?.map((p) =>
          p.id === provider ? { ...p, connected: false, account: null } : p,
        ) ?? null,
    }));
  },
});
