import { Notice } from "../../components/ui/Notice";
import { useStore } from "../../store";

/** What the host's idle sweep moved to History while nobody asked it to
 *  (`workspace:auto-archived`). The list already dropped them; this says where
 *  they went. Up until dismissed. */
export function AutoArchivedNotice() {
  const names = useStore((s) => s.autoArchived);
  const dismiss = useStore((s) => s.dismissAutoArchived);
  if (!names || names.length === 0) return null;
  const n = names.length;
  return (
    <Notice icon="archive" className="home-note" onDismiss={dismiss}>
      Archived {n} idle workspace{n === 1 ? "" : "s"}: {names.join(", ")}. Restore from History on
      your Mac.
    </Notice>
  );
}
