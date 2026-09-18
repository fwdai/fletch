import { describe, expect, it } from "vitest";
import { pairTargetFrom } from "./pairLink";

const LINK =
  "fletch://pair?host=Zm9vYmFy&addr=10.0.0.4:47285&relay=wss%3A%2F%2Frelay.fletch.sh&token=K7PQ2M9X&name=Cloud%20box";

describe("the Paired hosts field", () => {
  it("accepts a pairing link and keeps the host key, address, relay and name", () => {
    expect(pairTargetFrom(` ${LINK} `)).toEqual({
      target: {
        host: "10.0.0.4",
        port: 47285,
        hostKey: "Zm9vYmFy",
        pairingToken: "K7PQ2M9X",
        relay: "wss://relay.fletch.sh",
        name: "Cloud box",
      },
    });
  });

  it("refuses anything that is not a pairing link", () => {
    for (const input of ["", "   ", "10.0.0.4:47285", "https://fletch.sh", "fletch://open?x=1"]) {
      const result = pairTargetFrom(input);
      expect(result).toHaveProperty("error");
      expect("error" in result && result.error).toMatch(/isn't a Fletch pairing link/);
    }
  });

  it("refuses a link with no address to dial", () => {
    expect(pairTargetFrom("fletch://pair?host=Zm9vYmFy&token=K7PQ2M9X")).toHaveProperty("error");
  });

  it("refuses a link with no host key: the key is the environment id here", () => {
    const result = pairTargetFrom("fletch://pair?addr=10.0.0.4:47285&token=K7PQ2M9X");

    expect("error" in result && result.error).toMatch(/no host key/);
  });

  it("refuses a link with no pairing code", () => {
    const result = pairTargetFrom("fletch://pair?host=Zm9vYmFy&addr=10.0.0.4:47285");

    expect("error" in result && result.error).toMatch(/no pairing code/);
  });
});
