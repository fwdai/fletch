import { useAppStore } from "@/store";
import { hostAccountNote } from "@/store/capabilities";

/** The line the GitHub and Linear groups show while any host is paired: these
 *  sign-ins are This Mac's, and a host signs in on its own. Renders nothing
 *  while no host is paired. */
export function HostAccountNote() {
  const environments = useAppStore((s) => s.environments);
  const note = hostAccountNote(Object.values(environments));
  if (!note) return null;
  return <div className="set-gh-hint text-sm">{note}</div>;
}
