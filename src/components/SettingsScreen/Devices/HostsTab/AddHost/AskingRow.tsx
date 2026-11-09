import { Button } from "@/components/ui/Button";
import type { NearbyHost } from "@/remote/nearby";
import { SetRow } from "../../../primitives";

/** A confirmed pairing in flight: asking, then the six digits both screens
 *  show while the host's owner decides. They must match there; that match is
 *  what proves nobody is in the middle (docs/remote-protocol.md, "Confirmed
 *  pairing"). */
export function AskingRow({
  host,
  code,
  onCancel,
}: {
  host: NearbyHost;
  /** Null until the host has answered the request. */
  code: string | null;
  onCancel: () => void;
}) {
  return (
    <SetRow
      title={`Pairing with ${host.name}`}
      sub={
        code ? (
          <>
            Accept on {host.name} when it shows{" "}
            <span className="set-pair-digits mono">
              {code.slice(0, 3)} {code.slice(3)}
            </span>
          </>
        ) : (
          `Asking ${host.name}…`
        )
      }
    >
      <Button variant="ghost" size="sm" onClick={onCancel}>
        Cancel
      </Button>
    </SetRow>
  );
}
