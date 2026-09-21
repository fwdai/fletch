import { hostProvidersNote } from "@/store/capabilities";
import type { EnvironmentEntry } from "@/store/environments";

/** One line naming the providers a paired host cannot run, with the fix for
 *  each in its tooltip. Sits beside `HostGateNote`, which says the same kind of
 *  thing about ops: that one is "this host is too old to do X", this one is
 *  "this host has no signed-in X to do it with".
 *
 *  Renders nothing for the local environment, for a host that has not answered
 *  `host_providers` (not greeted yet, or too old for the op), and for a host
 *  whose providers are all installed and signed in — so both surfaces that
 *  identify a host drop it in unconditionally.
 *
 *  `className` is the caller's own sub-line class: the note is a line in an
 *  existing stack, not a component with a look of its own. */
export function HostProvidersNote({
  env,
  className,
}: {
  env: EnvironmentEntry;
  className: string;
}) {
  const note = hostProvidersNote(env);
  if (!note) return null;
  return (
    <span className={`${className} tip`} data-tip={note.tip}>
      {note.summary}
    </span>
  );
}
