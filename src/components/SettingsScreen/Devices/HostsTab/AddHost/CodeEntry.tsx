import { useState } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import type { NearbyHost } from "@/remote/nearby";
import { SetRow } from "../../../primitives";

/** For a host with nobody at its screen to accept (a headless `fletch-host`):
 *  the one-time code it shows instead, typed here. */
export function CodeEntry({
  host,
  sending,
  onSubmit,
  onCancel,
}: {
  host: NearbyHost;
  sending: boolean;
  onSubmit: (code: string) => void;
  onCancel: () => void;
}) {
  const [code, setCode] = useState("");
  const ready = !!code.trim() && !sending;

  return (
    <SetRow
      icon={<Icon name="laptop" size={16} />}
      title={host.name}
      sub="Nobody is at its screen to accept. Enter the code from fletch-host pair."
    >
      <input
        className="set-text-field set-pair-code-input mono text-sm"
        value={code}
        placeholder="K7PQ2M9X"
        spellCheck={false}
        autoCapitalize="characters"
        autoCorrect="off"
        // biome-ignore lint/a11y/noAutofocus: the row appears because its host was just picked, and the code is all it asks for
        autoFocus
        disabled={sending}
        aria-label="Pairing code"
        onChange={(e) => setCode(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && ready) onSubmit(code);
        }}
      />
      <Button variant="primary" size="sm" disabled={!ready} onClick={() => onSubmit(code)}>
        {sending ? "Pairing…" : "Pair"}
      </Button>
      <Button variant="ghost" size="sm" onClick={onCancel}>
        Cancel
      </Button>
    </SetRow>
  );
}
