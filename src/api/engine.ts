// An `Engine` is everything a caller needs to drive one environment: its ops
// (`api`) and its event streams (`events`), both bound to the same transport,
// and the id of the host they reach. Callers are handed one — store slices as
// their third argument, React code through `useEngine()` — instead of importing
// the `api` and `on*` of `@/api`.

import { createContext, useContext } from "react";
import {
  activeEnvironmentId,
  type EnvironmentId,
  environmentEntry,
  LOCAL_ENVIRONMENT_ID,
} from "@/store/environments";
import { type Events, makeEvents } from "./events";
import * as localBound from "./index";
import { type Api, makeApi } from "./index";
import { localTransport, type Transport } from "./transport";

export interface Engine {
  /** The environment this engine drives. */
  hostId: EnvironmentId;
  api: Api;
  events: Events;
}

/** An engine for the paired host `hostId`, over whichever transport `transport`
 *  answers with, asked on every call and every subscription. Local-only ops and
 *  events still reach this desktop (see `invokeLocal`, and `onLocal` in
 *  ./events). */
export function makeEngine(hostId: EnvironmentId, transport: () => Transport): Engine {
  return {
    hostId,
    api: makeApi((op, args) => transport().call(op, args), transport),
    events: makeEvents(
      (event, cb) => transport().on(event, cb),
      (event, cb) => localTransport.on(event, cb),
    ),
  };
}

/** This desktop's own engine. It is the `api` and the `on*` exports of `@/api`
 *  themselves, which are bound to `localTransport` — read when they are used
 *  rather than copied here, so whatever stands in for that module (a test's
 *  mock) is what this engine drives. */
export const localEngine: Engine = {
  hostId: LOCAL_ENVIRONMENT_ID,
  get api() {
    return localBound.api;
  },
  get events() {
    return localBound;
  },
};

/** What a host with no connection behind it answers: every call refuses, and
 *  nothing is ever heard. Only a host whose saved address cannot be dialled
 *  has no transport on its entry. */
const notConnected: Transport = {
  call: () => Promise.reject(new Error("This host is not connected.")),
  on: () => Promise.resolve(() => {}),
};

const remoteEngines = new Map<EnvironmentId, Engine>();

/** The engine of the environment `hostId`: this desktop's own, or a paired
 *  host's, made on first use over whatever transport the host's entry carries
 *  when it is asked (a re-pair replaces it). */
export function engineOf(hostId: EnvironmentId): Engine {
  if (hostId === LOCAL_ENVIRONMENT_ID) return localEngine;
  let engine = remoteEngines.get(hostId);
  if (!engine) {
    engine = makeEngine(hostId, () => environmentEntry(hostId).transport ?? notConnected);
    remoteEngines.set(hostId, engine);
  }
  return engine;
}

/** Forget a paired host's engine. */
export function dropEngine(hostId: EnvironmentId): void {
  remoteEngines.delete(hostId);
}

/** The engine of the environment on screen, resolved on every use — for the
 *  few callers outside React and outside the store that act on whatever the
 *  user is looking at (a global shortcut, telemetry, a provider sign-in). */
export const activeEngine: Engine = {
  get hostId() {
    return activeEnvironmentId();
  },
  get api() {
    return engineOf(activeEnvironmentId()).api;
  },
  get events() {
    return engineOf(activeEnvironmentId()).events;
  },
};

/** The engine the subtree drives, set with its host by `EngineScope`
 *  (@/store/EngineScope). Outside one, this desktop's own. */
export const EngineContext = createContext<Engine>(localEngine);

export function useEngine(): Engine {
  return useContext(EngineContext);
}
