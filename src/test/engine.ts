import { type Engine, localEngine } from "@/api/engine";
import { type EnvironmentId, LOCAL_ENVIRONMENT_ID } from "@/store/environments";

/** An engine for `hostId` that drives whatever stands in for `@/api` — a
 *  test's mock of it — so a slice under test can act as that host's. */
export function testEngine(hostId: EnvironmentId = LOCAL_ENVIRONMENT_ID): Engine {
  return {
    hostId,
    get api() {
      return localEngine.api;
    },
    get events() {
      return localEngine.events;
    },
  };
}
