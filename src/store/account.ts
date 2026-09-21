import { api, type GhStatus, type LinearStatus } from "@/api";
import { type AccountProfile, getAccount, saveAccountProfile, toProfile } from "@/storage/accounts";
import type { SliceCreator } from "./types";

export interface AccountSlice {
  /** Local account profile, loaded on init. `null` until the row is read. */
  account: AccountProfile | null;
  /** Anonymous usage telemetry consent. Opt-out: defaults on. */
  telemetryEnabled: boolean;
  /** Code-indexing (codegraph) consent. Opt-out: defaults on. */
  codeIndexingEnabled: boolean;
  /** Local (Whisper) dictation engine chosen over the platform recognizer.
   *  Opt-in: defaults off, since it costs a model download. */
  dictationEngineEnabled: boolean;
  /** Dictation ends itself after a pause. Opt-out: defaults on. Mirrors the
   *  backend-owned `dictation_auto_stop` setting. */
  dictationAutoStop: boolean;
  /** The ACTIVE environment's GitHub connection: null until the first probe,
   *  then the live status. `authenticated` gates push/PR/clone affordances
   *  app-wide, and those act where the checkout is — so on a paired host this
   *  is the host's login, not this Mac's. Settings › Account shows this Mac's
   *  own, which it probes for itself (`api.ghStatus`). */
  github: GhStatus | null;
  /** Linear connection: null until the first probe. `authenticated` gates
   *  Linear issue affordances (inbox rows, composer picker, team picker).
   *  Always this Mac's: the key is in this machine's keychain and no `linear_*`
   *  op is on the wire. */
  linear: LinearStatus | null;

  saveAccount: (patch: Pick<AccountProfile, "firstName" | "lastName" | "email">) => Promise<void>;
  /** Re-read the local account row into the store — e.g. after an OAuth
   *  sign-in writes the provider profile to SQLite. */
  refreshAccount: () => Promise<void>;
  /** Re-probe the GitHub connection into `github` (after sign-in/disconnect,
   *  and once on init). */
  refreshGithub: () => Promise<void>;
  /** Drop the stored GitHub token and return to local-only mode. */
  disconnectGithub: () => Promise<void>;
  /** Re-probe the Linear connection into `linear` (after connect/disconnect,
   *  and once on init). */
  refreshLinear: () => Promise<void>;
  setTelemetryEnabled: (enabled: boolean) => void;
  setCodeIndexingEnabled: (enabled: boolean) => void;
  /** Resolves `true` once the choice is persisted; `false` (with the store
   *  reverted) if the backend rejected it. */
  setDictationEngineEnabled: (enabled: boolean) => Promise<boolean>;
  setDictationAutoStop: (enabled: boolean) => void;
}

export const createAccountSlice: SliceCreator<AccountSlice> = (set, get) => ({
  account: null,
  telemetryEnabled: true,
  codeIndexingEnabled: true,
  dictationEngineEnabled: false,
  dictationAutoStop: true,
  github: null,
  linear: null,

  saveAccount: async (patch) => {
    const current = get().account;
    if (!current) return;
    try {
      await saveAccountProfile(current.id, patch);
      set({ account: { ...current, ...patch } });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },
  refreshAccount: async () => {
    try {
      const row = await getAccount();
      if (row) set({ account: toProfile(row) });
    } catch (e) {
      set({ lastError: String(e) });
    }
  },
  refreshGithub: async () => {
    try {
      set({ github: await api.ghStatusActive() });
    } catch {
      // A failed probe means we can't confirm a connection — treat as
      // not-connected so gated UI shows "connect" rather than a spinner.
      set({ github: { installed: true, authenticated: false, login: null } });
    }
  },
  refreshLinear: async () => {
    try {
      set({ linear: await api.linearStatus() });
    } catch {
      // A failed probe means we can't confirm a connection — treat as
      // not-connected so gated UI shows "connect" rather than a spinner.
      set({ linear: { authenticated: false, user: null } });
    }
  },
  disconnectGithub: async () => {
    try {
      await api.githubDisconnect();
      // Re-probe rather than assume: the token this cleared is this Mac's, and
      // `github` is the ACTIVE environment's answer — on a paired host it is
      // unchanged by the disconnect, and writing "not connected" there would
      // shut gates the host can still pass. A failed write leaves the token
      // stored, and the probe reports that too.
      await get().refreshGithub();
    } catch (e) {
      set({ lastError: String(e) });
    }
  },
  setTelemetryEnabled: (enabled) => {
    set({ telemetryEnabled: enabled });
    // The backend command persists the `telemetry_enabled` setting AND toggles
    // the live pipeline, so we don't also call setSetting here.
    void api.setTelemetryEnabled(enabled);
  },
  setCodeIndexingEnabled: (enabled) => {
    set({ codeIndexingEnabled: enabled });
    // The backend command persists `code_indexing_enabled` and (when enabling)
    // warms the index in the background, so we don't also call setSetting here.
    void api.setCodeIndexingEnabled(enabled);
  },
  setDictationEngineEnabled: async (enabled) => {
    const previous = get().dictationEngineEnabled;
    set({ dictationEngineEnabled: enabled });
    // The backend command persists `dictation_engine` and (when enabling)
    // downloads the model in the background, so we don't also call setSetting.
    // Optimistic, but not blindly: a failed write puts the toggle back, since
    // this one gates a half-gigabyte download and a delete.
    try {
      await api.setDictationEngine(enabled);
      return true;
    } catch {
      set({ dictationEngineEnabled: previous });
      return false;
    }
  },
  setDictationAutoStop: (enabled) => {
    set({ dictationAutoStop: enabled });
    // The backend command persists `dictation_auto_stop` and updates the
    // silence monitor's mirror, so no setSetting here.
    void api.setDictationAutoStop(enabled);
  },
});
