// The store as one host sees it.
//
// This desktop's own state is the store's top-level keys, read and written as
// they always were. A paired host's lives in its partition (`hostStates`, see
// ./hostState), and everything that acts for that host — its slice actions, its
// event listeners, a component rendered inside its `EngineScope` — goes through
// a scoped `get` / `set` instead of the store's own:
//
//   - `get` answers with a view: the whole store, with the host's partition and
//     the host's own actions laid over it. Memoised per state object, so a
//     selector run against it is as stable as one run against the store.
//   - `set` runs the update against that view, then splits what it wrote: the
//     host keys land in the host's partition, everything else (an error banner,
//     a busy flag, a screen opening) at the top level, where it always was.
//
// A host's actions are the slice creators run once more for it, over its scoped
// `get` / `set` and its engine, so an answer from that engine can only ever land
// in that host's partition. Nothing in a creator runs at creation time: every
// effect a slice has lives inside an action.

import { createContext, useContext } from "react";
import type { StoreApi } from "zustand";
import { dropEngine, type Engine, engineOf } from "@/api/engine";
import { type EnvironmentId, LOCAL_ENVIRONMENT_ID } from "./environments";
import { detachEventListeners, registerEventListeners, setupResync } from "./eventListeners";
import { BLANK_HOST_STATE, isHostKey } from "./hostState";
import { forgetHostState } from "./perHost";
import type { AppState } from "./types";

type SetState = StoreApi<AppState>["setState"];
type GetState = StoreApi<AppState>["getState"];

/** One host's `getState` / `setState`: the store's own for this desktop, the
 *  scoped pair for a paired host. */
export interface HostStore {
  getState: GetState;
  setState: SetState;
}

/** Builds every slice of the store over one `set` / `get` / engine. */
export type SlicesCreator = (set: SetState, get: GetState, engine: Engine) => AppState;

/** The host the subtree is scoped to (see `EngineScope`). Outside one, this
 *  desktop's own. */
export const HostContext = createContext<EnvironmentId>(LOCAL_ENVIRONMENT_ID);

export function useHostId(): EnvironmentId {
  return useContext(HostContext);
}

export function createHostScope(store: StoreApi<AppState>, slices: SlicesCreator) {
  const local: HostStore = { getState: store.getState, setState: store.setState };
  const views = new Map<EnvironmentId, { state: AppState; view: AppState }>();
  const actions = new Map<EnvironmentId, Partial<AppState>>();
  const stores = new Map<EnvironmentId, HostStore>();
  /** The hosts whose listeners are attached, with what stops their resync. */
  const started = new Map<EnvironmentId, () => void>();

  /** The store as `hostId` sees it. This desktop's is the state itself. */
  function viewOf(state: AppState, hostId: EnvironmentId): AppState {
    if (hostId === LOCAL_ENVIRONMENT_ID) return state;
    const cached = views.get(hostId);
    if (cached?.state === state) return cached.view;
    const view: AppState = {
      ...state,
      ...(state.hostStates[hostId] ?? BLANK_HOST_STATE),
      ...actionsOf(hostId),
    };
    views.set(hostId, { state, view });
    return view;
  }

  /** The slice actions of `hostId`, bound to its scoped `get` / `set` and its
   *  engine. Built once per published host; one for a host that is not (or no
   *  longer) published is built for the caller and not kept. */
  function actionsOf(hostId: EnvironmentId): Partial<AppState> {
    const cached = actions.get(hostId);
    if (cached) return cached;
    const { getState, setState } = hostStore(hostId);
    const all = slices(setState, getState, engineOf(hostId)) as unknown as Record<string, unknown>;
    const own: Record<string, unknown> = {};
    for (const key of Object.keys(all)) {
      if (typeof all[key] === "function") own[key] = all[key];
    }
    if (store.getState().environments[hostId]) actions.set(hostId, own);
    return own as Partial<AppState>;
  }

  /** `partial`, run against `hostId`'s view, as a write to the store: the host
   *  keys it changed into the host's partition, the rest at the top level.
   *  Keys it hands back unchanged are left out, so an update that returns the
   *  whole view (a no-op) writes nothing. A host forgotten meanwhile gets
   *  nothing written to a partition it no longer has. */
  function writeAs(
    state: AppState,
    hostId: EnvironmentId,
    partial: Parameters<SetState>[0],
  ): AppState | Partial<AppState> {
    const view = viewOf(state, hostId);
    const next = typeof partial === "function" ? partial(view) : partial;
    if (!next) return state;
    const was = view as unknown as Record<string, unknown>;
    const top: Record<string, unknown> = {};
    const own: Record<string, unknown> = {};
    let touched = false;
    let ownTouched = false;
    for (const [key, value] of Object.entries(next)) {
      if (Object.is(value, was[key])) continue;
      if (isHostKey(key)) {
        own[key] = value;
        ownTouched = true;
      } else {
        top[key] = value;
        touched = true;
      }
    }
    const partition = state.hostStates[hostId];
    if (ownTouched && partition) {
      top.hostStates = { ...state.hostStates, [hostId]: { ...partition, ...own } };
      touched = true;
    }
    return touched ? (top as Partial<AppState>) : state;
  }

  /** `hostId`'s `getState` / `setState`, for code outside React that acts for
   *  one host. This desktop's are the store's own. */
  function hostStore(hostId: EnvironmentId): HostStore {
    if (hostId === LOCAL_ENVIRONMENT_ID) return local;
    let scoped = stores.get(hostId);
    if (!scoped) {
      scoped = {
        getState: () => viewOf(store.getState(), hostId),
        setState: ((partial: Parameters<SetState>[0]) =>
          store.setState((state) => writeAs(state, hostId, partial))) as SetState,
      };
      stores.set(hostId, scoped);
    }
    return scoped;
  }

  /** Make a paired host live: its event listeners folding into its partition,
   *  and its resync on focus. Once per host until it is stopped; this
   *  desktop's own are attached by `init`. */
  async function startHost(hostId: EnvironmentId): Promise<void> {
    if (hostId === LOCAL_ENVIRONMENT_ID || started.has(hostId)) return;
    const { getState, setState } = hostStore(hostId);
    const engine = engineOf(hostId);
    started.set(hostId, setupResync(engine, setState, getState));
    await registerEventListeners(engine, setState, getState);
  }

  /** Forget a paired host's listeners, actions, engine and per-host module
   *  state. Its partition goes with its entry (`removeEnvironment`). */
  async function stopHost(hostId: EnvironmentId): Promise<void> {
    if (hostId === LOCAL_ENVIRONMENT_ID) return;
    started.get(hostId)?.();
    started.delete(hostId);
    views.delete(hostId);
    actions.delete(hostId);
    stores.delete(hostId);
    dropEngine(hostId);
    forgetHostState(hostId);
    await detachEventListeners(hostId);
  }

  return { viewOf, hostStore, startHost, stopHost };
}
