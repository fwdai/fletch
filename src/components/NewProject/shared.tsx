import type { GhStatus } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { DeviceCode } from "@/components/ui/DeviceCode";
import { ModalBody } from "@/components/ui/Modal";
import { Spinner } from "@/components/ui/Spinner";
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
    <div className="modal-field">
      <label className="modal-label text-sm">Location</label>
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
  const { connect, cancel, device, error, busy } = useGithubConnect(undefined, "new_project");
  return (
    <ModalBody>
      <div className="np-gate flex-center">
        <Icon name="github" size={22} />
        {device ? (
          <>
            <div className="np-gate-t text-base">Finish signing in in your browser</div>
            <DeviceCode code={device.userCode} verificationUri={device.verificationUri} />
            <Button variant="link" size="sm" onClick={cancel}>
              Cancel
            </Button>
          </>
        ) : error ? (
          <>
            <div className="np-gate-t text-base">Sign-in failed</div>
            <div className="np-gate-s text-sm">{error}</div>
            <Button variant="primary" size="lg" onClick={() => void connect()}>
              Try again
            </Button>
          </>
        ) : (
          <>
            <div className="np-gate-t text-base">Connect GitHub to {what}</div>
            <div className="np-gate-s text-sm">
              Fletch works fully offline for local projects. Connect GitHub when you want to clone,
              push, or open pull requests.
            </div>
            <Button variant="primary" size="lg" disabled={!!busy} onClick={() => void connect()}>
              {busy ? (
                <>
                  <Spinner /> Connecting…
                </>
              ) : (
                <>
                  <Icon name="github" size={14} /> Connect GitHub
                </>
              )}
            </Button>
          </>
        )}
      </div>
    </ModalBody>
  );
}
