import { DEFAULT_PORT, type HostTarget } from "./types";

/** Parse a `fletch://pair?host=…&port=…&token=…&name=…` deep link (or a bare
 *  `host[:port]`) into a connect target. Returns null when it isn't one.
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
  const host = params.get("host")?.trim();
  const token = params.get("token")?.trim();
  if (!host) return null;
  const port = Number(params.get("port")) || DEFAULT_PORT;
  const target: HostTarget = { host, port };
  if (token) target.pairingToken = token;
  const name = params.get("name")?.trim();
  if (name) target.name = name;
  return target;
}

/** `ws://<host>:<port>/ws`. IPv6 literals need bracketing. */
export function wsUrl(target: Pick<HostTarget, "host" | "port">): string {
  const host = target.host.includes(":") ? `[${target.host}]` : target.host;
  return `ws://${host}:${target.port}/ws`;
}
