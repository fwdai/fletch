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
   *  token goes to the backend and is held nowhere here. An empty `account`
   *  is the provider's default. */
  connectCloud: (provider: CloudProviderId, token: string, account: string) => Promise<void>;
  /** Delete the provider's spare machines, then forget its token. Rejects
   *  with the refusal to show, the list unchanged, while a spare could not be
   *  deleted. The machines of workspaces there stay where they are. */
  disconnectCloud: (provider: CloudProviderId) => Promise<void>;
  /** How many spare machines each connected provider keeps, as stored on
   *  this Mac, `""` for the engine's default; `null` until the first read. */
  cloudSpares: string | null;
  /** Re-read `cloudSpares`. Rejects with the reason, keeping the last value. */
  refreshCloudSpares: () => Promise<void>;
  /** Store a new count, `""` for the default. Resolves to the engine's
   *  refusal to show, or `null` once the count is stored. */
  saveCloudSpares: (count: string) => Promise<string | null>;
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

// A connect or a disconnect adds or takes away a workspace type (one per
// connected provider), so the composer's answer is read again after either.
export const createCloudSlice: SliceCreator<CloudSlice> = (set, get, { api }) => ({
  cloudProviders: null,

  refreshCloudProviders: async () => {
    set({ cloudProviders: await api.cloudProviders() });
  },

  connectCloud: async (provider, token, account) => {
    const status = await api.cloudConnect(provider, token, account);
    set((s) => ({ cloudProviders: withCloudProvider(s.cloudProviders, status) }));
    get().reloadWorkspaceTypes();
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
    get().reloadWorkspaceTypes();
  },

  cloudSpares: null,

  refreshCloudSpares: async () => {
    set({ cloudSpares: (await api.cloudSpares()) ?? "" });
  },

  saveCloudSpares: async (count) => {
    try {
      await api.cloudSetSpares(count || null);
      set({ cloudSpares: count });
      return null;
    } catch (e) {
      return String(e);
    }
  },
});
