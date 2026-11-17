// One link per host: a protocol client and the api over it, kept by host key so
// everything that talks to a host reaches it through the same connection. Links
// are built on demand and kept until dropped. Nothing here dials: which link is
// connected, and what it says to the screen, is the store's business.

import { type Api, createApi } from "../api";
import { clientFactory, type RemoteClient } from "../remote";

/** One host's connection, and the api over it. */
export interface HostLink {
  /** The host it talks to. Null only on a link from `pairingLink` whose host
   *  has not answered yet. */
  readonly hostKey: string | null;
  readonly client: RemoteClient;
  readonly api: Api;
}

interface Link extends HostLink {
  hostKey: string | null;
}

/** Read when the app loads, so every link talks to the same kind of host. */
const newClient = clientFactory();

const links = new Map<string, Link>();

function build(hostKey: string | null): Link {
  const client = newClient();
  return { hostKey, client, api: createApi(client) };
}

/** The link for `hostKey`: the same one every time, built the first time it is
 *  asked for. */
export function linkOf(hostKey: string): HostLink {
  let link = links.get(hostKey);
  if (!link) {
    link = build(hostKey);
    links.set(hostKey, link);
  }
  return link;
}

/** A link for a pairing that does not know its host yet — an address typed by
 *  hand, whose key the client pins on first contact. It belongs to no host
 *  until that host answers; from then on it is that host's link, and `linkOf`
 *  hands it back in place of any the host had before, which is dropped.
 *
 *  Taken over on the first snapshot rather than when `connect` resolves: a
 *  pairing that spent its code and lost the socket before the workspace came
 *  is completed by the client's own retry, with no `connect` above it. */
export function pairingLink(): HostLink {
  const link = build(null);
  const off = link.client.onSnapshot(() => {
    const hostKey = link.client.hostKey;
    if (!hostKey) return;
    off();
    if (links.get(hostKey) !== link) dropLink(hostKey);
    link.hostKey = hostKey;
    links.set(hostKey, link);
  });
  return link;
}

/** Stop a host's link for good and forget it: the next `linkOf` for that host
 *  builds a new one. */
export function dropLink(hostKey: string): void {
  links.get(hostKey)?.client.disconnect();
  links.delete(hostKey);
}
