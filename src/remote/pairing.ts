import { DEFAULT_PORT, type HostTarget } from "./types";

/** `<ip-or-name>[:port]`, or `[<ipv6>][:port]`. A bare IPv6 literal is
 *  accepted too: more than one colon and no brackets can only be an address,
 *  since a port would be ambiguous. */
export function parseAddress(input: string): { host: string; port: number } | null {
  const text = input.trim();
  if (!text) return null;
  const bracketed = /^\[([^\]]+)\](?::(\d+))?$/.exec(text);
  if (bracketed) return { host: bracketed[1], port: Number(bracketed[2]) || DEFAULT_PORT };
  const colons = text.split(":").length - 1;
  if (colons > 1) return { host: text, port: DEFAULT_PORT };
  const [host, port] = text.split(":");
  if (!host) return null;
  return { host, port: Number(port) || DEFAULT_PORT };
}

/** Parse a `fletch://pair?host=<host public key>&addr=<ip>:<port>
 *  &relay=<url-encoded relay base URL>&token=<code>&name=<name>` deep link into
 *  a connect target. Returns null when it isn't
 *  one — the deep-link handler connects on anything this accepts, so a bare
 *  address is `parseAddress`'s business, not this one's.
 *
 *  Accepts the `fletch:` scheme with or without the `//`, since iOS pasteboards
 *  and QR readers both turn up. `URL` can't be trusted to expose `searchParams`
 *  for a non-special scheme in every engine, so the query is split by hand. */
export function parsePairUrl(input: string): HostTarget | null {
  const text = input.trim();
  if (!text) return null;
  const match = /^fletch:(?:\/\/)?pair\?(.*)$/i.exec(text);
  if (!match) return null;
  const params = new URLSearchParams(match[1]);
  const addr = params.get("addr")?.trim();
  const address = addr ? parseAddress(addr) : null;
  if (!address) return null;
  const target: HostTarget = address;
  // `host` is the host's public key — its identity, and the phone's
  // authentication of the Mac.
  const hostKey = params.get("host")?.trim();
  if (hostKey) target.hostKey = hostKey;
  const token = params.get("token")?.trim();
  if (token) target.pairingToken = token;
  const name = params.get("name")?.trim();
  if (name) target.name = name;
  // `relay=` is url-encoded in the link and present only when the host has a
  // relay configured; `URLSearchParams` has already decoded it.
  const relay = params.get("relay")?.trim();
  if (relay) target.relay = relay;
  return target;
}

/** `ws://<host>:<port>/ws`. IPv6 literals need bracketing. */
export function wsUrl(target: Pick<HostTarget, "host" | "port">): string {
  const host = target.host.includes(":") ? `[${target.host}]` : target.host;
  return `ws://${host}:${target.port}/ws`;
}

/** `<relay>/v1/device/<hostKey>` — the phone's endpoint on the relay
 *  (docs/remote-protocol.md, "Relay"). The host key is the route, and it is
 *  already base64url, so nothing needs encoding. */
export function relayDeviceUrl(relay: string, hostKey: string): string {
  return `${relay.trim().replace(/\/+$/, "")}/v1/device/${hostKey}`;
}
