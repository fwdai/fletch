import { type HostTone, hostTone } from "@/remote/hostTone";
import type { EnvironmentEntry } from "@/store/environments";

/** A host's connection dot, by its tone (`hostTone`): green live, amber
 *  dialling, grey out of reach, red only for what the user has to clear. */
export function HostDot({ tone, className }: { tone: HostTone; className?: string }) {
  return <i className={className ? `env-dot ${className}` : "env-dot"} data-state={tone} />;
}

/** The dot of a host this app has an entry for. */
export function EnvironmentDot({ env }: { env: EnvironmentEntry }) {
  return <HostDot tone={hostTone(env.connection, env.retrying)} />;
}
