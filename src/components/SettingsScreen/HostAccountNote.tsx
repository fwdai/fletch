import { useAppStore } from "@/store";
import { activeEntry, hostAccountNote } from "@/store/capabilities";

/** The line the GitHub and Linear groups show while a paired host is the
 *  active environment: these sign-ins are this Mac's, whatever the switcher
 *  says. Renders nothing for the local environment. */
export function HostAccountNote() {
  const note = useAppStore((s) => hostAccountNote(activeEntry(s)));
  if (!note) return null;
  return <div className="set-gh-hint text-sm">{note}</div>;
}
