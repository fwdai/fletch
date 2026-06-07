import type { GhStatus } from "../../api";
import { Icon } from "../Icon";

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
      <button className="np-dest" onClick={onPick}>
        <Icon name="folder" size={14} />
        {parent ? (
          <span className="np-dest-path">
            {trimmed}
            {name ? (
              <span className="np-dest-name">
                {sep}
                {name}
              </span>
            ) : null}
          </span>
        ) : (
          <span className="np-dest-empty">Choose a folder…</span>
        )}
        <Icon name="chevR" size={13} />
      </button>
    </div>
  );
}

/** Shown when `gh` is missing or logged out — both flows need it. */
export function GhGate({ gh }: { gh: GhStatus }) {
  return (
    <div className="np-body">
      <div className="np-gate">
        <Icon name="github" size={22} />
        {!gh.installed ? (
          <>
            <div className="np-gate-t">GitHub CLI not found</div>
            <div className="np-gate-s">
              Install the <code>gh</code> CLI to clone and create GitHub
              repositories, then reopen this dialog.
            </div>
          </>
        ) : (
          <>
            <div className="np-gate-t">Not signed in to GitHub</div>
            <div className="np-gate-s">
              Run <code>gh auth login</code> in your terminal, then reopen this
              dialog.
            </div>
          </>
        )}
      </div>
    </div>
  );
}
