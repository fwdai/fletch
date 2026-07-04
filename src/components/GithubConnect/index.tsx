import { useEffect, useRef } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { CopyButton } from "@/components/ui/CopyButton";
import { Scrim } from "@/components/ui/Scrim";
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
  const { connect, cancel, device, error, busy } = useGithubConnect(close);

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
    <>
      <Scrim onClose={onClose} zIndex={400} />
      <div className="ghc-modal" role="dialog" aria-modal="true">
        <div className="ghc-h flex-center text-base">
          <Icon name="github" size={15} />
          <span>Connect GitHub</span>
          <button className="ghc-close flex-center" aria-label="Close" onClick={onClose}>
            <Icon name="close" size={14} />
          </button>
        </div>

        <div className="ghc-body">
          {device ? (
            <>
              <div className="ghc-lede text-sm">
                Finish signing in in the browser tab that just opened, then enter this code:
              </div>
              <div className="ghc-code-row flex-center">
                <span className="ghc-code text-2xl">{device.userCode}</span>
                <CopyButton text={device.userCode} />
              </div>
              <div className="ghc-uri mono text-sm">{device.verificationUri}</div>
              <Button variant="outline" size="sm" onClick={onClose}>
                Cancel
              </Button>
            </>
          ) : error ? (
            <>
              <div className="ghc-title text-base">Couldn’t connect</div>
              <div className="ghc-err text-sm">{error}</div>
              <div className="ghc-actions flex-center">
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
              <Icon name="refresh" size={14} className="ghc-spin" />
              Starting GitHub sign-in…
            </div>
          )}
        </div>
      </div>
    </>
  );
}
