import type { GhStatus } from "@/api";
import { Icon } from "@/components/Icon";
import { useGithubConnect } from "@/util/useGithubConnect";

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

/** Connect-GitHub prompt for a flow that genuinely needs it (cloning). Runs
 *  the device flow inline: on success the store's `github` flips and the
 *  parent view re-renders to the real form — no dialog reopen. */
export function ConnectGitHub({ what }: { what: string }) {
  const { connect, cancel, device, error, busy } = useGithubConnect();
  return (
    <div className="np-body">
      <div className="np-gate flex-center">
        <Icon name="github" size={22} />
        {device ? (
          <>
            <div className="np-gate-t text-base">Finish signing in in your browser</div>
            <div className="np-gate-code text-2xl">{device.userCode}</div>
            <div className="np-gate-s text-sm">{device.verificationUri}</div>
            <button className="np-link text-sm" onClick={cancel}>
              Cancel
            </button>
          </>
        ) : error ? (
          <>
            <div className="np-gate-t text-base">Sign-in failed</div>
            <div className="np-gate-s text-sm">{error}</div>
            <button className="np-primary flex-center text-base" onClick={() => void connect()}>
              Try again
            </button>
          </>
        ) : (
          <>
            <div className="np-gate-t text-base">Connect GitHub to {what}</div>
            <div className="np-gate-s text-sm">
              Fletch works fully offline for local projects. Connect GitHub when you want to clone,
              push, or open pull requests.
            </div>
            <button
              className="np-primary flex-center text-base"
              disabled={!!busy}
              onClick={() => void connect()}
            >
              {busy ? (
                <>
                  <Icon name="refresh" size={13} /> Connecting…
                </>
              ) : (
                <>
                  <Icon name="github" size={14} /> Connect GitHub
                </>
              )}
            </button>
          </>
        )}
      </div>
    </div>
  );
}
