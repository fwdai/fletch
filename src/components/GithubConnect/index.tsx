import { useEffect, useRef } from "react";
import { Button } from "@/components/ui/Button";
import { DeviceCode } from "@/components/ui/DeviceCode";
import { Modal, ModalBody } from "@/components/ui/Modal";
import { Spinner } from "@/components/ui/Spinner";
import { useAppStore } from "@/store";
import { useGithubConnect } from "@/util/useGithubConnect";

/** App-level GitHub connect modal. Any "Connect GitHub" affordance opens it via
 *  `openGithubConnect()` and the OAuth device flow starts immediately — one
 *  click to begin, with the device code / any config error shown right here
 *  instead of behind a detour into Settings. Mounted once at the app root;
 *  renders nothing until opened. Closes itself on a successful connection. */
export function GithubConnectModal() {
  const open = useAppStore((s) => s.githubConnectOpen);
  const close = useAppStore((s) => s.closeGithubConnect);
  const { connect, cancel, device, error, busy } = useGithubConnect(close, "connect_gate");

  // Kick off the flow once per open. We deliberately depend on `open` alone and
  // reach `connect` through a ref: `connect`'s identity changes on every `busy`
  // transition, so listing it here would re-run the effect mid-flow (e.g. right
  // after a failure clears `busy`) — the `started` guard would still hold, but
  // it's cleaner not to fire at all. `started` (reset only when the modal
  // closes) also stops a dev-mode double-invoke from starting two attempts.
  const connectRef = useRef(connect);
  connectRef.current = connect;
  const started = useRef(false);
  useEffect(() => {
    if (!open) {
      started.current = false;
      return;
    }
    if (started.current) return;
    started.current = true;
    void connectRef.current();
  }, [open]);

  if (!open) return null;

  const onClose = () => {
    cancel();
    close();
  };

  return (
    <Modal icon="github" title="Connect GitHub" onClose={onClose} size="sm" layer="overlay">
      <ModalBody center>
        {device ? (
          <>
            <div className="ghc-lede text-sm">
              Finish signing in in the browser tab that just opened, then enter this code:
            </div>
            <DeviceCode code={device.userCode} verificationUri={device.verificationUri} />
            <Button variant="outline" size="sm" onClick={onClose}>
              Cancel
            </Button>
          </>
        ) : error ? (
          <>
            <div className="ghc-title text-base">Couldn’t connect</div>
            <div className="modal-error text-sm">{error}</div>
            <div className="modal-actions">
              <Button variant="primary" disabled={!!busy} onClick={() => void connect()}>
                Try again
              </Button>
              <Button variant="outline" onClick={onClose}>
                Close
              </Button>
            </div>
          </>
        ) : (
          <div className="ghc-lede flex-center text-sm">
            <Spinner size={14} />
            Starting GitHub sign-in…
          </div>
        )}
      </ModalBody>
    </Modal>
  );
}
