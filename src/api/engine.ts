// An `Engine` is everything a caller needs to drive one environment: its ops
// (`api`) and its event streams (`events`), both bound to the same transport.
// Callers are handed one — store slices as their third argument, React code
// through `useEngine()` — instead of importing the `api` and `on*` of `@/api`.

import { createContext, useContext } from "react";
import { type Events, makeEvents } from "./events";
import * as activeBound from "./index";
import { type Api, makeApi } from "./index";
import { localTransport, type Transport } from "./transport";

export interface Engine {
  api: Api;
  events: Events;
}

/** An engine over whichever transport `transport` answers with, asked on every
 *  call and every subscription. Local-only ops and events still reach this
 *  desktop (see `invokeLocal`, and `onLocal` in ./events). */
export function makeEngine(transport: () => Transport): Engine {
  return {
    api: makeApi((op, args) => transport().call(op, args)),
    events: makeEvents(
      (event, cb) => transport().on(event, cb),
      (event, cb) => localTransport.on(event, cb),
    ),
  };
}

/** The engine of the environment the UI is driving — the only one there is.
 *  It is the `api` and the `on*` exports of `@/api` themselves, read when they
 *  are used rather than copied here, so whatever stands in for that module (a
 *  test's mock) is what this engine drives. */
export const activeEngine: Engine = {
  get api() {
    return activeBound.api;
  },
  get events() {
    return activeBound;
  },
};

/** The engine the subtree drives. No provider is mounted, so this is
 *  `activeEngine` everywhere. */
export const EngineContext = createContext<Engine>(activeEngine);

export function useEngine(): Engine {
  return useContext(EngineContext);
}
