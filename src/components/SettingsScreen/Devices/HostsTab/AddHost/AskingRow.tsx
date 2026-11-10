import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import type { NearbyHost } from "@/remote/nearby";
import { SetRow } from "../../../primitives";

/** A confirmed pairing in flight, on the host's own row: asking, then the six
 *  digits both screens show while the host's owner decides. They must match
 *  there; that match is what proves nobody is in the middle
 *  (docs/remote-protocol.md, "Confirmed pairing"). */
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
      icon={<Icon name="laptop" size={16} />}
      title={host.name}
      sub={code ? "Accept on that Mac if it shows this code." : "Asking…"}
    >
      {code && (
        <span className="set-pair-digits mono" role="status" aria-label={`Code ${code}`}>
          {code.slice(0, 3)} {code.slice(3)}
        </span>
      )}
      <Button variant="ghost" size="sm" onClick={onCancel}>
        Cancel
      </Button>
    </SetRow>
  );
}
