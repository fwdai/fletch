// The Fletch remote relay. A dumb pipe: it routes by host ID, verifies one
// DH proof on the host link, and multiplexes device links onto it. Everything
// it carries is already ciphertext from the end-to-end Noise channel.
//
// See ../../docs/remote-protocol.md, section "Relay".

import type { ApnsEnv } from "./apns";
import { decodeHostKey, HOST_ID_LENGTH } from "./auth";
import { HostRelay } from "./host-relay";

export { HostRelay };

/** The APNs secrets are optional: without them the relay ignores NOTIFY. */
export interface Env extends ApnsEnv {
  HOSTS: DurableObjectNamespace;
}

// `wss://<relay>/v1/host/<hostId>` and `wss://<relay>/v1/device/<hostId>`.
// Anything else is 404.
const ROUTE = new RegExp(`^/v1/(host|device)/([A-Za-z0-9_-]{${HOST_ID_LENGTH}})$`);

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const match = ROUTE.exec(url.pathname);
    if (!match) return new Response("not found", { status: 404 });

    const role = match[1];
    const hostId = match[2];
    // The charset and length are in the pattern; this rejects the remaining
    // 43-character strings whose trailing bits are non-canonical, so one host
    // ID cannot be spelled two ways and land in two Durable Objects.
    if (!decodeHostKey(hostId)) return new Response("not found", { status: 404 });

    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
      return new Response("expected a websocket upgrade", { status: 426 });
    }

    const target = new URL(request.url);
    // `set` overwrites, so a client cannot smuggle in its own role.
    target.searchParams.set("role", role);
    target.searchParams.set("hostId", hostId);
    return env.HOSTS.get(env.HOSTS.idFromName(hostId)).fetch(new Request(target, request));
  },
};
