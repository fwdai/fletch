// The per-host links (src/host): one client and api per host key, and the
// hand-over that makes a pairing by address the link of the host it reached.
// The jsdom URL in vite.config.ts puts this file in mock mode, so a link's
// client dials the in-memory host, which answers as `MOCK_HOST_KEY`.

import { describe, expect, it } from "vitest";
import { dropLink, linkOf, pairingLink } from "../src/host";
import { DEFAULT_PORT } from "../src/remote";
import { MOCK_HOST_KEY } from "../src/remote/mock";

describe("links by host key", () => {
  it("hands back the same link for the same host", () => {
    const link = linkOf("host-a");
    expect(linkOf("host-a")).toBe(link);
    expect(link.hostKey).toBe("host-a");
  });

  it("gives another host a link of its own, client and api included", () => {
    const a = linkOf("host-a");
    const b = linkOf("host-b");
    expect(b).not.toBe(a);
    expect(b.client).not.toBe(a.client);
    expect(b.api).not.toBe(a.api);
  });

  it("stops a dropped link and never hands it back", async () => {
    const dropped = linkOf("host-c");
    await dropped.client.connect({ host: "mock", port: DEFAULT_PORT, hostKey: "host-c" });
    expect(dropped.client.state).toBe("connected");

    dropLink("host-c");
    expect(dropped.client.state).toBe("disconnected");
    const fresh = linkOf("host-c");
    expect(fresh).not.toBe(dropped);
    expect(fresh.client).not.toBe(dropped.client);
  });
});

describe("a pairing by address", () => {
  it("belongs to no host until one answers, then to that host in place of its old link", async () => {
    const old = linkOf(MOCK_HOST_KEY);
    await old.client.connect({ host: "mock", port: DEFAULT_PORT, hostKey: MOCK_HOST_KEY });
    const pairing = pairingLink();
    expect(pairing.hostKey).toBeNull();
    expect(linkOf(MOCK_HOST_KEY)).toBe(old);

    await pairing.client.connect({ host: "mock", port: DEFAULT_PORT });

    expect(pairing.hostKey).toBe(MOCK_HOST_KEY);
    expect(linkOf(MOCK_HOST_KEY)).toBe(pairing);
    // One link per host: the one it replaced is stopped, not left running.
    expect(old.client.state).toBe("disconnected");
    pairing.client.disconnect();
  });
});
