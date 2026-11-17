// What the tests share: the mock host brought up as `init` brings it up, host
// events delivered as a socket would, paired hosts seeded on disk, and the
// waits and hand-released answers the cases are built from.

import { createElement, type ReactElement } from "react";
import { expect, vi } from "vitest";
import { type HostLink, linkOf } from "../src/host";
import { OnHost } from "../src/host/OnHost";
import { MOCK_HOST_KEY } from "../src/remote/mock";
import { appStore, type HostStore, hostStoreOf } from "../src/store";
import type { SavedHost } from "../src/store/persist";

/** Two hosts the mock answers as themselves once `registerMockHost` has given
 *  them an identity — and as the mock host until then. */
export const MINI = "bWluaQ";
export const STUDIO = "c3R1ZGlv";

/** The two as the settings file keeps them once paired. */
export const PAIRED_MINI: SavedHost = {
  hostKey: MINI,
  name: "Mac mini",
  host: "10.0.0.5",
  port: 47285,
  pairedAt: "2026-01-01",
};
export const PAIRED_STUDIO: SavedHost = {
  hostKey: STUDIO,
  name: "Studio Mac",
  host: "10.0.0.7",
  port: 47285,
  pairedAt: "2026-02-01",
};

/** The store of the host on screen. */
export function hostOnScreen(): HostStore {
  const { hostKey } = appStore.getState();
  if (!hostKey) throw new Error("no host is on screen");
  return hostStoreOf(hostKey);
}

/** `element` acting on `hostKey`'s host, as the app renders a screen or sheet. */
export const onHost = (hostKey: string, element: ReactElement): ReactElement =>
  createElement(OnHost, { hostKey }, element);

/** `init` in the suite's mock mode, once the mock host it pairs with has
 *  connected: that host's link. */
export async function launchMock(): Promise<HostLink> {
  await appStore.getState().init();
  await vi.waitFor(() => expect(hostOnScreen().getState().connection).toBe("connected"), {
    timeout: 5000,
  });
  return linkOf(MOCK_HOST_KEY);
}

/** Deliver a host event frame to `hostKey`'s link — the mock host's unless
 *  another is named — as its socket would: for the events the mock never
 *  emits on its own. */
export function hostEvent(event: string, payload: unknown, hostKey = MOCK_HOST_KEY): void {
  (
    linkOf(hostKey).client as unknown as {
      dispatchEvent(frame: { event: string; payload: unknown }): void;
    }
  ).dispatchEvent({ event, payload });
}

/** Out of the suite's `?mock=1`, so `init` reads the paired hosts off disk and
 *  dials them as real ones. Returns what puts the URL back. */
export function leaveMockMode(): () => void {
  const url = window.location.href;
  window.history.replaceState({}, "", "/");
  return () => window.history.replaceState({}, "", url);
}

/** `hosts` paired on disk with `active` the one the app was left on, and mock
 *  mode left (`leaveMockMode`). Returns what puts the URL back. */
export function pairedOnDisk(hosts: SavedHost[], active: string): () => void {
  localStorage.setItem("fletch-mobile-settings", JSON.stringify({ hosts, activeHostKey: active }));
  return leaveMockMode();
}

/** Back in the foreground, as the webview says so. */
export const foreground = () => document.dispatchEvent(new Event("visibilitychange"));

/** A moment for work that was fired and not awaited — what "nothing happened"
 *  needs before it can be checked. */
export const settle = (ms = 30) => new Promise((resolve) => setTimeout(resolve, ms));

/** An answer held back until the test lets it land. */
export function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}
