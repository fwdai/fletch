import type { EnvironmentEntry } from "@/store/environments";

/** The connection dot, sharing the settings pane's colours so one host reads
 *  the same in both places. A host that is retrying is amber like a first dial
 *  — something is happening — while one that has stopped is red. */
export function EnvironmentDot({ env }: { env: EnvironmentEntry }) {
  const state =
    env.connection === "connected" || env.connection === "connecting"
      ? env.connection
      : env.retrying
        ? "connecting"
        : "error";
  return <i className="env-dot" data-state={state} />;
}
