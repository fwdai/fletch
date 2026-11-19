import type {
  CloudProviderId,
  CloudProviderStatus,
  CloudRunnerDeleted,
  CloudRunnerProgress,
  CloudRunnerRecord,
  CloudRunnerStatus,
} from "@/api";
import { thisDevice } from "@/remote/device";
import type { SliceCreator } from "./types";

/** A runner create this window started and that has not finished: the
 *  provider it is at, and the steps the engine has reported, in order. */
export interface CloudRunnerCreate {
  provider: CloudProviderId;
  steps: CloudRunnerProgress[];
}

export interface CloudSlice {
  /** The cloud providers on this Mac, one per provider the backend knows, and
   *  whether each is connected. `null` until the first read. Always this
   *  Mac's, like `linear`: the tokens are in this machine's keychain and no
   *  `cloud_*` op is on the wire, so it is not a host key — whatever offers a
   *  workspace in the cloud reads the answer Settings › Cloud writes. */
  cloudProviders: CloudProviderStatus[] | null;
  /** This install's runner, as `cloud_runner_status` last answered. `null`
   *  until the first read. This Mac's, like `cloudProviders`. */
  cloudRunner: CloudRunnerStatus | null;
  /** The runner create under way from this window, if one is. Here rather
   *  than in the pane so its steps, and the GitHub code among them, outlive
   *  the pane. */
  cloudRunnerCreate: CloudRunnerCreate | null;

  /** Re-read `cloudProviders`. Rejects with the reason, keeping the last list. */
  refreshCloudProviders: () => Promise<void>;
  /** Check `token` with the provider and keep it; the provider's entry takes
   *  the answer. Rejects with the provider's reason, the list unchanged. The
   *  token goes to the backend and is held nowhere here. An empty `account`
   *  is the provider's default. */
  connectCloud: (provider: CloudProviderId, token: string, account: string) => Promise<void>;
  /** Delete the provider's spare machines, then forget its token. Rejects
   *  with the refusal to show, the list unchanged, while a spare could not be
   *  deleted or this install's runner is at the provider. The machines of
   *  workspaces there stay where they are. */
  disconnectCloud: (provider: CloudProviderId) => Promise<void>;
  /** How many spare machines each connected provider keeps, as stored on
   *  this Mac, `""` for the engine's default; `null` until the first read. */
  cloudSpares: string | null;
  /** Re-read `cloudSpares`. Rejects with the reason, keeping the last value. */
  refreshCloudSpares: () => Promise<void>;
  /** Store a new count, `""` for the default. Resolves to the engine's
   *  refusal to show, or `null` once the count is stored. */
  saveCloudSpares: (count: string) => Promise<string | null>;

  /** Re-read `cloudRunner`. Rejects with the reason, keeping the last answer.
   *  Each answer is what the saved hosts are made to agree with
   *  (`followRunnerStatus` in @/remote/hosts, which follows `cloudRunner`), so
   *  a create or a delete, which both end by reading it, needs nothing more to
   *  put the runner among the paired hosts or take it out. */
  refreshCloudRunner: () => Promise<void>;
  /** Create the runner at `provider`, authorising this device on it, and
   *  resolve with its record; `cloudRunnerCreate` holds the steps meanwhile.
   *  Rejects with the engine's reason, which has undone what it made. The
   *  Claude token goes to the backend and is held nowhere here. A blank
   *  `name` is the engine's default. */
  createCloudRunner: (
    provider: CloudProviderId,
    claudeToken: string,
    name: string,
  ) => Promise<CloudRunnerRecord>;
  /** Start the runner's machine; resolves once it runs. */
  startCloudRunner: () => Promise<void>;
  /** Stop the runner's machine; resolves once it has stopped. */
  stopCloudRunner: () => Promise<void>;
  /** Delete the runner. Rejects with the engine's refusal of one it cannot
   *  reach unless `force`. */
  deleteCloudRunner: (force: boolean) => Promise<CloudRunnerDeleted>;
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
// Every runner action reads the runner's status again when it ends, whether it
// worked or not: either way the runner may have moved.
export const createCloudSlice: SliceCreator<CloudSlice> = (set, get, { api, events }) => {
  /** `action`, then a fresh status, which does not replace its outcome. */
  const thenRefresh = async <T>(action: () => Promise<T>): Promise<T> => {
    try {
      return await action();
    } finally {
      await get()
        .refreshCloudRunner()
        .catch(() => {});
    }
  };

  return {
    cloudProviders: null,
    cloudRunner: null,
    cloudRunnerCreate: null,

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

    refreshCloudRunner: async () => {
      set({ cloudRunner: await api.cloudRunnerStatus() });
    },

    // Subscribed before the call, so no step is missed. The create is cleared
    // only once the fresh status is in, so the pane goes from the steps
    // straight to the runner (or back to the form) with nothing in between.
    createCloudRunner: async (provider, claudeToken, name) => {
      if (get().cloudRunnerCreate) throw new Error("A runner is being created already.");
      set({ cloudRunnerCreate: { provider, steps: [] } });
      let off = () => {};
      try {
        off = await events.onCloudRunnerProgress((step) =>
          set((s) =>
            s.cloudRunnerCreate
              ? {
                  cloudRunnerCreate: {
                    ...s.cloudRunnerCreate,
                    steps: [...s.cloudRunnerCreate.steps, step],
                  },
                }
              : {},
          ),
        );
        const [devicePublicKey, device] = await Promise.all([
          api.remoteDevicePublicKey(),
          thisDevice(),
        ]);
        return await thenRefresh(() =>
          api.cloudRunnerCreate(provider, {
            devicePublicKey,
            deviceName: device.name,
            devicePlatform: device.platform,
            claudeToken,
            name: name.trim() || null,
          }),
        );
      } finally {
        off();
        set({ cloudRunnerCreate: null });
      }
    },

    startCloudRunner: () => thenRefresh(() => api.cloudRunnerStart()),

    stopCloudRunner: () => thenRefresh(() => api.cloudRunnerStop()),

    deleteCloudRunner: (force) => thenRefresh(() => api.cloudRunnerDelete(force)),
  };
};
