import { Notice } from "../../components/ui/Notice";
import { severalHosts, useApp, useHostName, useHostStore } from "../../store";

/** What the idle sweep of the host this acts on moved to History while nobody
 *  asked it to (`workspace:auto-archived`). The list already dropped them;
 *  this says where they went — on which Mac, when there are several. Up until
 *  dismissed. */
export function AutoArchivedNotice() {
  const names = useHostStore((s) => s.autoArchived);
  const dismiss = useHostStore((s) => s.dismissAutoArchived);
  const several = useApp(severalHosts);
  const host = useHostName();
  if (!names || names.length === 0) return null;
  const n = names.length;
  return (
    <Notice icon="archive" className="home-note" onDismiss={dismiss}>
      Archived {n} idle workspace{n === 1 ? "" : "s"}: {names.join(", ")}. Restore from History on{" "}
      {several && host ? host : "your Mac"}.
    </Notice>
  );
}
