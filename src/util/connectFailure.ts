/** Why a GitHub device-flow sign-in failed, as a category safe to report.
 *
 *  The raw error is never sent (it can carry a URL, an account handle or a
 *  provider's own wording), so this is what the funnel sees. The strings
 *  matched here are the ones `oauth::device_login` (Rust) and the desktop
 *  shell produce; anything else lands in `other`. */
export type ConnectFailureReason =
  /** The build carries no OAuth client id (a local dev build, typically). */
  | "not_configured"
  /** The provider refused to issue a device code at all. */
  | "device_code_rejected"
  /** The user never finished in the browser before the code lapsed. */
  | "expired"
  /** The user declined the authorization in the browser. */
  | "denied"
  /** The provider rejected the poll for another reason (bad client, flow
   *  disabled, wrong code). */
  | "provider_error"
  /** Signed in, but the token could not be written to the secret store. */
  | "token_store"
  /** A request never reached the provider, or timed out. */
  | "network"
  /** The provider answered with something that could not be read. */
  | "bad_response"
  | "other";

export function connectFailureReason(error: string): ConnectFailureReason {
  const msg = error.toLowerCase();
  if (msg.includes("not configured")) return "not_configured";
  if (msg.includes("device code request rejected")) return "device_code_rejected";
  if (msg.includes("device code expired")) return "expired";
  if (msg.includes("couldn't save your github login")) return "token_store";
  if (msg.includes("authorization failed")) {
    if (msg.includes("expired_token")) return "expired";
    if (msg.includes("access_denied")) return "denied";
    return "provider_error";
  }
  if (msg.includes("malformed token response") || msg.includes("error decoding response")) {
    return "bad_response";
  }
  if (
    msg.includes("error sending request") ||
    msg.includes("dns error") ||
    msg.includes("connection") ||
    msg.includes("timed out") ||
    msg.includes("tls")
  ) {
    return "network";
  }
  return "other";
}
