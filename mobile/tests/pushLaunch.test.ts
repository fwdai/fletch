// A tapped alert that launched the app, from a paired host that is not the one
// it was left on: it is held until `init` has read the paired hosts, then
// opens its agent on that host at once — before any host has answered — and
// that host is dialled once, like every other.
//
// Its own file because `init` runs once per module registry, and this is about
// what it finds queued.

import { describe, expect, it, vi } from "vitest";
import { linkOf } from "../src/host";
import { registerMockHost } from "../src/remote/mock";
import { appStore, hostStoreOf } from "../src/store";

const MINI = "bWluaQ";
const STUDIO = "c3R1ZGlv";

const app = () => appStore.getState();

describe("an alert that launched the app", () => {
  it("opens on its own host at once, which is dialled once and read again when it answers", async () => {
    registerMockHost(STUDIO, { host: { name: "Studio Mac", appVersion: "0.7.23", os: "macos" } });
    localStorage.setItem(
      "fletch-mobile-settings",
      JSON.stringify({
        hosts: [
          {
            hostKey: MINI,
            name: "Mac mini",
            host: "10.0.0.5",
            port: 47285,
            pairedAt: "2026-01-01",
          },
          {
            hostKey: STUDIO,
            name: "Studio Mac",
            host: "10.0.0.7",
            port: 47285,
            pairedAt: "2026-02-01",
          },
        ],
        activeHostKey: MINI,
      }),
    );
    const connects = vi.spyOn(linkOf(STUDIO).client, "connect");
    const url = window.location.href;
    window.history.replaceState({}, "", "/");

    // How the studio's connection stood when the agent's screen went up.
    let studioWas: string | null = null;
    const off = appStore.subscribe((s) => {
      if (studioWas === null && s.nav.at(-1)?.props.agentId === "kamakura") {
        studioWas = hostStoreOf(STUDIO).getState().connection;
      }
    });
    app().openFromPush({ hostId: STUDIO, agentId: "kamakura", kind: "turn_complete" });
    expect(app().ready).toBe(false);
    await app().init();
    off();

    // On the studio with the agent open before it had answered.
    expect(studioWas).toBe("connecting");
    expect(app().hostKey).toBe(STUDIO);
    expect(app().nav.at(-1)?.props).toEqual({ agentId: "kamakura", hostKey: STUDIO });
    expect(connects).toHaveBeenCalledOnce();
    await vi.waitFor(() => {
      expect(hostStoreOf(STUDIO).getState().connection).toBe("connected");
      expect(hostStoreOf(MINI).getState().connection).toBe("connected");
      expect(hostStoreOf(STUDIO).getState().logLoads.kamakura).toEqual({ status: "ready" });
    });
    // Still open on the studio: answering does not reset the stack.
    expect(app().nav.at(-1)?.props).toEqual({ agentId: "kamakura", hostKey: STUDIO });
    window.history.replaceState({}, "", url);
  });
});
