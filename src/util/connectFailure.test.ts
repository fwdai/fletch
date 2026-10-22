import { describe, expect, it } from "vitest";
import { connectFailureReason } from "./connectFailure";

describe("connectFailureReason", () => {
  it("names the build-time gap", () => {
    expect(connectFailureReason("GitHub sign-in is not configured")).toBe("not_configured");
  });

  it("separates the two ways a code can lapse from a refusal", () => {
    expect(connectFailureReason("device code expired before authorization")).toBe("expired");
    expect(connectFailureReason("authorization failed: expired_token")).toBe("expired");
    expect(connectFailureReason("authorization failed: access_denied")).toBe("denied");
    expect(connectFailureReason("authorization failed: device_flow_disabled")).toBe(
      "provider_error",
    );
  });

  it("tells a provider refusal from a transport failure", () => {
    expect(
      connectFailureReason("device code request rejected: invalid_client bad id (raw: {})"),
    ).toBe("device_code_rejected");
    expect(connectFailureReason("error sending request for url (https://x)")).toBe("network");
    expect(connectFailureReason("operation timed out")).toBe("network");
    expect(connectFailureReason("error decoding response body")).toBe("bad_response");
  });

  it("flags a login that succeeded but could not be kept", () => {
    expect(connectFailureReason("couldn't save your GitHub login: keychain locked")).toBe(
      "token_store",
    );
  });

  it("never leaks: anything unrecognised is just other", () => {
    expect(connectFailureReason("https://github.com/octocat went wrong")).toBe("other");
  });
});
