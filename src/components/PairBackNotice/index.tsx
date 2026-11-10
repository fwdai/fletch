import { useEffect } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui";
import { useInviteOffers } from "./useInviteOffers";

/** How long the notice stays before leaving on its own: news, not a question. */
const LINGER_MS = 8_000;

/** Bottom-left toast once this Mac has paired with a desktop that offered its
 *  own link (docs/remote-protocol.md, "Pairing back"): the other half of the
 *  pairing someone here just accepted. */
export function PairBackNotice() {
  const { controlling, dismiss } = useInviteOffers();

  useEffect(() => {
    if (!controlling) return;
    const t = window.setTimeout(dismiss, LINGER_MS);
    return () => window.clearTimeout(t);
  }, [controlling, dismiss]);

  if (!controlling) return null;
  return (
    <div className="docker-build-stack">
      <div className="update-toast docker-build-toast" role="status">
        <Icon name="laptop" />
        <div className="update-toast-body">
          <div className="update-toast-text">
            <strong>Now controlling “{controlling}” too</strong>
            <span>Switch to it from the top of the sidebar.</span>
          </div>
          <div className="update-toast-actions">
            <Button variant="primary" onClick={dismiss}>
              OK
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}
