import { useState } from "react";
import { Button } from "@/components/ui/Button";
import { SetDisclosure, SetRow } from "../../../primitives";
import type { AddHost } from "../usePairedHosts";
import { pairTargetFrom } from "./pairLink";

/** The way in for a host the nearby list cannot see — another network, a
 *  cloud box: paste the `fletch://pair?…` link it minted. Behind a disclosure,
 *  since on a shared network picking the host is quicker. */
export function PasteLink({ add }: { add: AddHost }) {
  const [link, setLink] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const submit = async () => {
    const parsed = pairTargetFrom(link);
    if ("error" in parsed) {
      setError(parsed.error);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await add(parsed.target);
      setLink("");
    } catch (e) {
      // The client has already turned the host's or the transport's words into
      // something presentable.
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <SetDisclosure label="Paste a pairing link">
      <SetRow
        title="Pairing link"
        sub="For a host on another network or a cloud box. Copy it from the host's Settings › Devices › Clients, or from fletch-host pair."
        align="start"
      >
        <input
          className="set-text-field set-pair-link mono text-sm"
          value={link}
          placeholder="fletch://pair?…"
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          disabled={busy}
          aria-label="Pairing link"
          onChange={(e) => {
            setLink(e.target.value);
            setError(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") void submit();
          }}
        />
        <Button variant="primary" disabled={busy || !link.trim()} onClick={() => void submit()}>
          {busy ? "Pairing…" : "Add host"}
        </Button>
      </SetRow>
      {error && <div className="set-inline-warn">{error}</div>}
    </SetDisclosure>
  );
}
