import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui";

/** A checkout whose own git config names programs git would run. Fletch will not
 *  run git there (`git::hardening`), so this replaces the panel body: it names
 *  the keys and offers to remove them. */
export function BlockedConfigCard({
  keys,
  busy,
  onRemove,
}: {
  keys: string[];
  busy: boolean;
  onRemove: () => void;
}) {
  return (
    <div className="git-banner att text-base git-blocked">
      <div className="flex-center git-blocked-h">
        <Icon name="alert" size={14} />
        <span>Git is paused in this checkout: its config tells git to run a program.</span>
      </div>
      <span className="mono text-xs">{keys.join(", ")}</span>
      <span className="text-sm">
        A setup script (git-lfs, say) usually wrote these, but an agent could have too. Fletch won't
        commit, diff or push here until they're gone.
      </span>
      <Button variant="outline" size="sm" disabled={busy} onClick={onRemove}>
        Remove these settings
      </Button>
    </div>
  );
}
