import type { GhStatus } from "@/api";
import { Icon } from "@/components/Icon";

/** Shared state passed from the modal shell to each view. */
export interface NewProjectShared {
  parent: string;
  setParent: (p: string) => void;
  pickParent: () => Promise<void>;
  gh: GhStatus | null;
}

/** Destination-folder row: shows the chosen parent and, when known, the final
 *  `<parent>/<name>` path so the user sees exactly where the repo lands. */
export function DestRow({
  parent,
  onPick,
  name,
}: {
  parent: string;
  onPick: () => void;
  name?: string;
}) {
  const sep = parent.includes("\\") ? "\\" : "/";
  const trimmed = parent.replace(/[/\\]+$/, "");
  return (
    <div className="np-field">
      <label>Location</label>
      <button className="np-dest flex-center" onClick={onPick}>
        <Icon name="folder" size={14} />
        {parent ? (
          <span className="np-dest-path text-base">
            {trimmed}
            {name ? (
              <span className="np-dest-name">
                {sep}
                {name}
              </span>
            ) : null}
          </span>
        ) : (
          <span className="np-dest-empty text-base">Choose a folder…</span>
        )}
        <Icon name="chevR" size={13} />
      </button>
    </div>
  );
}

/** Shown when the app has no GitHub connection — both flows need one.
 *  (`gh.installed` is always true now that GitHub goes through the API; the
 *  only gate left is authentication.) */
export function GhGate(_props: { gh: GhStatus }) {
  return (
    <div className="np-body">
      <div className="np-gate flex-center">
        <Icon name="github" size={22} />
        <div className="np-gate-t text-base">Not connected to GitHub</div>
        <div className="np-gate-s text-sm">
          Sign in with GitHub from Settings → Account (or the onboarding tour), then reopen this
          dialog.
        </div>
      </div>
    </div>
  );
}
