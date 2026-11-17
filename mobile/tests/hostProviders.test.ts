// Which providers a host can run (lib/hostProviders) is read of the host the
// component acts on — and a component that comes to act on another host shows
// nothing of the first while the second has yet to answer. Nothing here dials:
// each host's answer is a spy on its link's api.

import { act, createElement } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import { linkOf } from "../src/host";
import { OnHost } from "../src/host/OnHost";
import { useHostProviders } from "../src/lib/hostProviders";
import type { HostProtocol, HostProvider } from "../src/remote";
import { hostStoreOf } from "../src/store";

const PROTOCOL: HostProtocol = { version: 3, ops: ["host_providers"], events: [], features: [] };

const seen: (HostProvider[] | null)[] = [];

function Probe() {
  seen.push(useHostProviders(true));
  return null;
}

it("never answers one host's rows for another", async () => {
  hostStoreOf("providers-a").setState({ protocol: PROTOCOL });
  hostStoreOf("providers-b").setState({ protocol: PROTOCOL });
  const onA = [{ id: "claude", installed: true, auth: "signed_in" }] as unknown as HostProvider[];
  vi.spyOn(linkOf("providers-a").api, "hostProviders").mockResolvedValue(onA);
  // The other host has yet to answer.
  vi.spyOn(linkOf("providers-b").api, "hostProviders").mockReturnValue(new Promise(() => {}));

  const node = document.createElement("div");
  const root = createRoot(node);
  const on = (hostKey: string) =>
    act(async () =>
      root.render(createElement(OnHost, { hostKey, children: createElement(Probe) })),
    );

  await on("providers-a");
  expect(seen.at(-1)).toEqual(onA);
  await on("providers-b");
  expect(seen.at(-1)).toBeNull();
  act(() => root.unmount());
});
