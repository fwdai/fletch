import { describe, expect, it } from "vitest";
import { nearbyTarget } from "./nearby";

// 0x0123456789abcdef then zeros, base64url: `fletch-0123456789abcdef.local`.
const KEY = "ASNFZ4mrze8AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

describe("dialling a nearby host", () => {
  it("dials the resolved address, pinning the announced key", () => {
    expect(
      nearbyTarget({ name: "Studio", hostKey: KEY, port: 47285, addresses: ["10.0.0.7"] }),
    ).toEqual({ host: "10.0.0.7", port: 47285, hostKey: KEY, name: "Studio" });
  });

  it("dials the key's .local name when there is no address, as the phone does", () => {
    expect(nearbyTarget({ name: "Studio", hostKey: KEY, port: 47285 })?.host).toBe(
      "fletch-0123456789abcdef.local",
    );
  });

  it("refuses a key no name derives from", () => {
    expect(nearbyTarget({ name: "Odd", hostKey: "not base64!", port: 1 })).toBeNull();
  });
});
