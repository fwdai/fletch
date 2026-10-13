// The Providers screen end to end over the mock host: the real protocol
// client, the real store slice, the real screen. The jsdom URL in
// vite.config.ts puts this file in mock mode (see `mockEnabled`).

import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { providerLimits } from "../src/remote/mock/fixtures";
import { ProvidersScreen } from "../src/screens/Providers";
import { client, useStore } from "../src/store";

const state = () => useStore.getState();

/** Deliver a host event frame to the store's listeners, as the socket would. */
const hostEvent = (event: string, payload: unknown) =>
  (
    client as unknown as { dispatchEvent(frame: { event: string; payload: unknown }): void }
  ).dispatchEvent({ event, payload });

let host: HTMLDivElement;
let root: Root;

beforeAll(async () => {
  await state().init();
  await vi.waitFor(() => expect(state().connection).toBe("connected"), { timeout: 5000 });
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => {
    root.render(createElement(ProvidersScreen));
  });
  await vi.waitFor(() => expect(host.querySelectorAll(".pv-acct").length).toBe(4));
});

afterAll(() => {
  act(() => root.unmount());
  host.remove();
});

const cards = () => [...host.querySelectorAll<HTMLElement>(".pv-acct")];
const cardOf = (label: string) =>
  cards().find((c) => c.querySelector(".lbl")?.textContent === label) as HTMLElement;
const mainOf = (label: string) => cardOf(label).querySelector<HTMLButtonElement>(".pv-acct-main");

describe("the host chip's providers screen", () => {
  it("lists every account under its provider, the default first, with the active one checked", () => {
    const names = [...host.querySelectorAll(".pv-provider-name")].map((n) => n.textContent);
    expect(names).toEqual(["Claude Code", "Codex"]);
    expect(cards().map((c) => c.querySelector(".lbl")?.textContent)).toEqual([
      "Terminal login",
      "work",
      "Terminal login",
      "personal",
    ]);
    expect(cards().map((c) => c.classList.contains("on"))).toEqual([false, true, true, false]);
  });

  it("draws each account's meters and reset times from the host's rows", () => {
    const work = cardOf("work");
    const pcts = [...work.querySelectorAll(".pv-limit-pct")].map((n) => n.textContent);
    expect(pcts).toEqual(["88%", "64%", "41%"]);
    expect(work.querySelector(".pv-limit.warn")).not.toBeNull();
    expect(work.textContent).toContain("resets ");
    expect(work.textContent).toContain("agent session");
    // The never-read, signed-out account says what would get it a reading.
    expect(cardOf("personal").textContent).toContain("Sign in on the Mac");
  });

  it("cannot select the active account or one the host would refuse", () => {
    expect(mainOf("work")?.disabled).toBe(true);
    expect(mainOf("personal")?.disabled).toBe(true);
    expect(cardOf("personal").textContent).toContain("Not signed in");
    expect(mainOf("Terminal login")?.disabled).toBe(false);
  });

  it("moves the radio on tap and the screen follows the host's answer", async () => {
    await act(async () => {
      mainOf("Terminal login")?.click();
    });
    await vi.waitFor(() => {
      const claude = state().providerAccounts.claude;
      expect(claude.find((a) => a.id === "default")?.active).toBe(true);
      expect(claude.find((a) => a.id === "work")?.active).toBe(false);
    });
    await vi.waitFor(() =>
      expect(cards().map((c) => c.classList.contains("on"))).toEqual([true, false, true, false]),
    );
  });

  it("follows a selection written elsewhere through settings:changed", async () => {
    await act(async () => {
      hostEvent("settings:changed", { key: "provider_account_claude", value: "work" });
    });
    expect(state().providerAccounts.claude.find((a) => a.id === "work")?.active).toBe(true);
    await vi.waitFor(() => expect(cardOf("work").classList.contains("on")).toBe(true));
  });

  it("folds a limits row an agent's session produced into the meters", async () => {
    const row = providerLimits(Date.now()).claude.default;
    const changed = {
      ...row,
      limits: {
        ...row.limits,
        five_hour: { percent: 97, resets_at: row.limits?.five_hour?.resets_at ?? null },
      },
    };
    await act(async () => {
      hostEvent("settings:changed", {
        key: "provider_limits_claude_default",
        value: JSON.stringify(changed),
      });
    });
    await vi.waitFor(() => {
      const card = cardOf("Terminal login");
      expect(card.querySelector(".pv-limit-pct")?.textContent).toBe("97%");
      expect(card.querySelector(".pv-limit.full")).not.toBeNull();
    });
  });

  it("refreshes every signed-in account of a provider on its button", async () => {
    const before = state().providerLimits.codex.default.limits?.as_of ?? 0;
    const button = [...host.querySelectorAll<HTMLButtonElement>(".sect-add")][1];
    await act(async () => {
      button.click();
    });
    await vi.waitFor(() => {
      expect(state().providerLimits.codex.default.limits?.as_of).toBeGreaterThanOrEqual(before);
      expect(state().providerLimits.codex.default.limits?.five_hour?.percent).toBe(24);
    });
    // The signed-out account was skipped, so it still has no row.
    expect(state().providerLimits.codex.personal).toBeUndefined();
  });
});
