// The Providers screen end to end over the mock host: the real protocol
// client, the real store slice, the real screen. The jsdom URL in
// vite.config.ts puts this file in mock mode (see `mockEnabled`).

import type { AccountLimits } from "@desktop/api/types/providers";
import { act, createElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { providerLimits } from "../src/remote/mock/fixtures";
import { ProvidersScreen } from "../src/screens/Providers";
import { api, client, useStore } from "../src/store";

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

describe("reading the accounts again", () => {
  it("keeps the last readings and reports a limits read that failed", async () => {
    const before = state().providerLimits;
    vi.spyOn(api, "getProviderLimits").mockRejectedValueOnce(new Error("The Mac did not answer"));
    await expect(state().loadProviderAccounts()).rejects.toThrow("The Mac did not answer");
    expect(state().providerAccountsLoad).toEqual({
      status: "error",
      error: "The Mac did not answer",
    });
    expect(state().providerLimits).toBe(before);
    await vi.waitFor(() => expect(host.textContent).toContain("The Mac did not answer"));
    expect(host.querySelectorAll(".pv-acct").length).toBe(4);

    // The next read clears it.
    await state().loadProviderAccounts();
    expect(state().providerAccountsLoad).toEqual({ status: "ready" });
    await vi.waitFor(() => expect(host.textContent).not.toContain("The Mac did not answer"));
  });

  it("applies a change that overtook the read over the read's answer", async () => {
    // The host probes each account before it answers; a selection written
    // elsewhere meanwhile is announced before the (older) list arrives.
    const list = api.listProviderAccounts;
    let release: () => void = () => {};
    vi.spyOn(api, "listProviderAccounts").mockImplementationOnce(async () => {
      await new Promise<void>((resolve) => {
        release = resolve;
      });
      return list();
    });
    // The mock host has claude on the default since the tap above; the event
    // says the selection has since moved to `work`.
    expect((await list()).find((a) => a.provider === "claude" && a.active)?.id).toBe("default");
    const read = state().loadProviderAccounts();
    expect(state().providerAccountsLoad).toEqual({ status: "loading" });
    hostEvent("settings:changed", { key: "provider_account_claude", value: "work" });
    release();
    await read;
    // The list's older answer (the default) did not undo the newer word.
    expect(state().providerAccounts.claude.find((a) => a.id === "work")?.active).toBe(true);
  });

  it("lands only the newest of two reads in flight", async () => {
    const list = api.listProviderAccounts;
    let releaseFirst: () => void = () => {};
    vi.spyOn(api, "listProviderAccounts").mockImplementationOnce(async () => {
      await new Promise<void>((resolve) => {
        releaseFirst = resolve;
      });
      // A stale answer: claude on `work`, where the host now has the default.
      return (await list()).map((a) =>
        a.provider === "claude" ? { ...a, active: a.id === "work" } : a,
      );
    });
    const first = state().loadProviderAccounts();
    const second = state().loadProviderAccounts();
    await second;
    expect(state().providerAccounts.claude.find((a) => a.id === "default")?.active).toBe(true);
    releaseFirst();
    await first;
    // The first read's stale answer never lands over the newer one's.
    expect(state().providerAccounts.claude.find((a) => a.id === "default")?.active).toBe(true);
    expect(state().providerAccountsLoad).toEqual({ status: "ready" });
  });

  it("does not let a refresh's answer put back a reading announced meanwhile", async () => {
    // The host stores and announces the row before it answers the refresh; an
    // agent's own session can announce a newer row in the same window.
    let release: (row: AccountLimits) => void = () => {};
    vi.spyOn(api, "refreshProviderLimits").mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          release = resolve;
        }),
    );
    const refresh = state().refreshProviderLimits("codex", "default");
    const stale = providerLimits(Date.now()).codex.default;
    const newer = {
      ...stale,
      limits: {
        ...stale.limits,
        as_of: (stale.limits?.as_of ?? 0) + 60,
        five_hour: { percent: 55, resets_at: null },
      },
    };
    hostEvent("settings:changed", {
      key: "provider_limits_codex_default",
      value: JSON.stringify(newer),
    });
    release(stale);
    await refresh;
    expect(state().providerLimits.codex.default.limits?.five_hour?.percent).toBe(55);
  });

  it("applies a refresh's answer that nothing overtook", async () => {
    const asked = Math.floor(Date.now() / 1000);
    await state().refreshProviderLimits("codex", "default");
    // The mock host announces and then answers the same row: either path
    // lands the host's reading, taken now, over the one the event above put.
    const row = state().providerLimits.codex.default.limits;
    expect(row?.five_hour?.percent).not.toBe(55);
    expect(row?.as_of).toBeGreaterThanOrEqual(asked);
  });
});
