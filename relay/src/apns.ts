// The relay's APNs sender, behind the NOTIFY frame. The provider key lives
// here and nowhere else, so a host only has to name the device tokens it wants
// alerted. See ../../docs/remote-protocol.md, "Push notifications".
//
// Fire and forget by design: nothing goes back to the host, Apple's errors are
// a log line, and there is no retry in v1. Nothing about a token is stored.

import { b64urlEncode, toBuffer } from "./auth";

/** One NOTIFY frame may name at most this many devices. */
export const MAX_TOKENS = 8;
/** `title` and `body` are shown to a human; anything longer is a bug. */
export const MAX_TEXT_CHARS = 200;
/** Apple caps `apns-collapse-id` at 64 bytes. */
const MAX_COLLAPSE_ID_CHARS = 64;
/** Apple accepts a provider token for 20–60 minutes; 50 leaves slack. */
export const JWT_TTL_MS = 50 * 60 * 1000;
/** An alert about a finished turn is worthless a day later. */
const EXPIRATION_SECONDS = 3600;

const PRODUCTION_HOST = "api.push.apple.com";
const SANDBOX_HOST = "api.sandbox.push.apple.com";

const encoder = new TextEncoder();
// Fatal, so a payload that is not valid UTF-8 is rejected rather than
// silently peppered with replacement characters.
const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: false });

export type ApnsEnvironment = "sandbox" | "production";

export interface PushTarget {
  token: string;
  environment: ApnsEnvironment;
}

export interface PushRequest {
  tokens: PushTarget[];
  title: string;
  body: string;
  kind: string;
  agentId: string;
  collapseId: string;
}

// -- the NOTIFY payload -----------------------------------------------------

export type ParsedPush = { ok: true; request: PushRequest } | { ok: false; reason: string };

const HEX = /^[0-9a-f]+$/;

/**
 * Validates a NOTIFY payload. Everything that reaches Apple as a URL path, a
 * header or user-visible text is bounded here; a rejection is only ever
 * ignored, so the reason exists for the log line.
 */
export function parsePushRequest(payload: Uint8Array): ParsedPush {
  let parsed: unknown;
  try {
    parsed = JSON.parse(decoder.decode(payload));
  } catch {
    return { ok: false, reason: "payload is not UTF-8 JSON" };
  }
  if (typeof parsed !== "object" || parsed === null) return { ok: false, reason: "not an object" };
  const raw = parsed as Record<string, unknown>;

  if (!Array.isArray(raw.tokens)) return { ok: false, reason: "tokens is not an array" };
  if (raw.tokens.length === 0 || raw.tokens.length > MAX_TOKENS) {
    return { ok: false, reason: `tokens has ${raw.tokens.length} entries` };
  }
  const tokens: PushTarget[] = [];
  for (const entry of raw.tokens) {
    const target = readTarget(entry);
    if (!target) return { ok: false, reason: "a token entry is malformed" };
    tokens.push(target);
  }

  const title = readText(raw.title);
  const body = readText(raw.body);
  const kind = readText(raw.kind);
  const agentId = readText(raw.agentId);
  if (title === null || body === null || kind === null || agentId === null) {
    return {
      ok: false,
      reason: `title, body, kind and agentId must be strings ≤ ${MAX_TEXT_CHARS}`,
    };
  }
  // An over-long or missing collapse id costs the coalescing, not the alert.
  const collapseId =
    typeof raw.collapseId === "string" && raw.collapseId.length <= MAX_COLLAPSE_ID_CHARS
      ? raw.collapseId
      : "";
  return { ok: true, request: { tokens, title, body, kind, agentId, collapseId } };
}

function readTarget(entry: unknown): PushTarget | null {
  if (typeof entry !== "object" || entry === null) return null;
  const { token, environment } = entry as { token?: unknown; environment?: unknown };
  // Lowercase hex is what makes a token safe to paste into a URL path. The
  // length is Apple's business (64 today), so only an absurd one is refused.
  if (typeof token !== "string" || token.length === 0 || token.length > 200) return null;
  if (!HEX.test(token)) return null;
  if (environment !== "sandbox" && environment !== "production") return null;
  return { token, environment };
}

function readText(value: unknown): string | null {
  if (typeof value !== "string" || value.length > MAX_TEXT_CHARS) return null;
  return value;
}

// -- configuration ----------------------------------------------------------

export interface ApnsEnv {
  APNS_TEAM_ID?: string;
  APNS_KEY_ID?: string;
  APNS_PRIVATE_KEY?: string;
  APNS_BUNDLE_ID?: string;
}

export interface ApnsConfig {
  teamId: string;
  keyId: string;
  /** PEM contents of the `.p8`: a PKCS#8 EC P-256 private key. */
  privateKey: string;
  bundleId: string;
}

/** Null unless all four secrets are set; a relay may run without push. */
export function readApnsConfig(env: ApnsEnv): ApnsConfig | null {
  const teamId = env.APNS_TEAM_ID?.trim();
  const keyId = env.APNS_KEY_ID?.trim();
  const privateKey = env.APNS_PRIVATE_KEY?.trim();
  const bundleId = env.APNS_BUNDLE_ID?.trim();
  if (!teamId || !keyId || !privateKey || !bundleId) return null;
  return { teamId, keyId, privateKey, bundleId };
}

// -- the client -------------------------------------------------------------

export type ApnsFetch = (request: Request) => Promise<Response>;

let currentFetch: ApnsFetch = (request) => fetch(request);

/** Test seam: the transport the Durable Object's own client will use. */
export function setApnsFetch(next: ApnsFetch): void {
  currentFetch = next;
}

export interface ApnsDeps {
  fetch?: ApnsFetch;
  now?: () => number;
}

export class ApnsClient {
  private readonly config: ApnsConfig;
  private readonly fetcher: ApnsFetch;
  private readonly now: () => number;
  private cached: { jwt: string; at: number } | null = null;

  constructor(config: ApnsConfig, deps: ApnsDeps = {}) {
    this.config = config;
    // Resolved per call, so a test can swap the transport after the Durable
    // Object has already built its client.
    this.fetcher = deps.fetch ?? ((request) => currentFetch(request));
    this.now = deps.now ?? Date.now;
  }

  /** Never throws: a push that cannot be sent is only a log line. */
  async send(hostId: string, request: PushRequest): Promise<void> {
    let jwt: string;
    try {
      jwt = await this.providerJwt();
    } catch (error) {
      console.log(`apns: cannot sign a provider token: ${error}`);
      return;
    }
    // The alert itself is the same for every device; only the URL differs.
    const body = JSON.stringify({
      aps: {
        alert: { title: request.title, body: request.body },
        sound: "default",
        "thread-id": request.agentId,
      },
      fletch: { hostId, agentId: request.agentId, kind: request.kind },
    });
    const expiration = Math.floor(this.now() / 1000) + EXPIRATION_SECONDS;
    await Promise.all(
      request.tokens.map((target) =>
        this.deliver(target, jwt, body, expiration, request.collapseId),
      ),
    );
  }

  private async deliver(
    target: PushTarget,
    jwt: string,
    body: string,
    expiration: number,
    collapseId: string,
  ): Promise<void> {
    const headers: Record<string, string> = {
      authorization: `bearer ${jwt}`,
      "content-type": "application/json",
      "apns-topic": this.config.bundleId,
      "apns-push-type": "alert",
      "apns-priority": "10",
      "apns-expiration": String(expiration),
    };
    if (collapseId) headers["apns-collapse-id"] = collapseId;
    const host = target.environment === "sandbox" ? SANDBOX_HOST : PRODUCTION_HOST;
    const url = `https://${host}/3/device/${target.token}`;
    try {
      const response = await this.fetcher(new Request(url, { method: "POST", headers, body }));
      if (response.ok) return;
      // 400 BadDeviceToken and 410 Unregistered mean the token is dead;
      // reporting that back to the host is a follow-up, not v1.
      console.log(`apns ${response.status} (${target.environment}): ${await readReason(response)}`);
    } catch (error) {
      console.log(`apns request failed (${target.environment}): ${error}`);
    }
  }

  private async providerJwt(): Promise<string> {
    const now = this.now();
    if (this.cached && now - this.cached.at < JWT_TTL_MS) return this.cached.jwt;
    const jwt = await signProviderJwt(this.config, Math.floor(now / 1000));
    this.cached = { jwt, at: now };
    return jwt;
  }
}

/** Apple's provider token: ES256 over `{alg, kid}` / `{iss, iat}`. */
export async function signProviderJwt(config: ApnsConfig, iat: number): Promise<string> {
  const header = jsonSegment({ alg: "ES256", kid: config.keyId });
  const claims = jsonSegment({ iss: config.teamId, iat });
  const input = `${header}.${claims}`;
  const key = await crypto.subtle.importKey(
    "pkcs8",
    toBuffer(pemToDer(config.privateKey)),
    { name: "ECDSA", namedCurve: "P-256" },
    false,
    ["sign"],
  );
  // WebCrypto signs ECDSA as raw r||s, which is exactly the JWS ES256 form —
  // no DER unwrapping, unlike most server-side crypto libraries.
  const signature = await crypto.subtle.sign(
    { name: "ECDSA", hash: "SHA-256" },
    key,
    toBuffer(encoder.encode(input)),
  );
  return `${input}.${b64urlEncode(new Uint8Array(signature))}`;
}

function jsonSegment(value: object): string {
  return b64urlEncode(encoder.encode(JSON.stringify(value)));
}

/** The `.p8` Apple hands out is standard base64 between PEM banners. */
function pemToDer(pem: string): Uint8Array {
  const base64 = pem.replace(/-----[^-]*-----/g, "").replace(/\s+/g, "");
  const raw = atob(base64);
  const out = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
  return out;
}

async function readReason(response: Response): Promise<string> {
  try {
    const body = (await response.json()) as { reason?: unknown };
    return typeof body.reason === "string" ? body.reason : "no reason";
  } catch {
    return "no reason";
  }
}
