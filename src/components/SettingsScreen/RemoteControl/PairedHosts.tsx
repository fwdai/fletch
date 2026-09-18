import { Button } from "@/components/ui/Button";
import { SetGroup, SetRow } from "../primitives";
import { HostRow } from "./HostRow";
import { usePairedHosts } from "./usePairedHosts";

/** Settings › Remote control › Paired hosts: the other side of this pane.
 *
 *  Everything above it is this Mac *as a host*. This is this Mac as a client:
 *  paste the pairing link another machine minted, and it is dialled in the
 *  background from now on, at every launch. Nothing here drives the UI yet —
 *  there is no host switcher until the next step — so a host that is offline
 *  costs nothing but its own dot. */
export function PairedHosts() {
  const { hosts, link, setLink, error, busy, add, forget } = usePairedHosts();

  return (
    <SetGroup label="Paired hosts" last>
      <SetRow
        title="Add a host"
        sub="Paste the pairing link from the other machine's Remote control settings. It is single use and expires in five minutes."
        align="start"
      >
        <input
          className="set-relay-url set-pair-link mono text-sm"
          value={link}
          placeholder="fletch://pair?…"
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          disabled={busy}
          aria-label="Pairing link"
          onChange={(e) => setLink(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void add();
          }}
        />
        <Button variant="primary" disabled={busy || !link.trim()} onClick={() => void add()}>
          {busy ? "Pairing…" : "Add host"}
        </Button>
      </SetRow>

      {error && <div className="set-inline-warn">{error}</div>}

      {hosts.length === 0 ? (
        <SetRow title="Hosts" sub="None yet. This Mac only runs its own agents." />
      ) : (
        hosts.map((host) => (
          <HostRow
            key={host.hostKey}
            host={host}
            disabled={busy}
            onForget={() => void forget(host.hostKey)}
          />
        ))
      )}
    </SetGroup>
  );
}
