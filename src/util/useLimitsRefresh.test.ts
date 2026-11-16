// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, type Mock, vi } from "vitest";
import type { ProviderAccount } from "@/api/types/providers";
import { limitsRefreshKey } from "@/data/providerLimits";
import { useAppStore } from "@/store";
import { act, renderHook } from "@/test/render";
import { useLimitsRefresh } from "./useLimitsRefresh";

const account = (
  provider: string,
  id: string,
  status: ProviderAccount["status"] = "signed_in",
): ProviderAccount => ({
  provider,
  id,
  managed: id !== "default",
  active: false,
  status,
  detail: null,
});

describe("useLimitsRefresh", () => {
  /** Each request waits here until the test settles it. */
  let settle: Record<string, { ok: () => void; fail: (why: string) => void }>;
  let refreshProviderLimits: Mock<(provider: string, account: string) => Promise<void>>;

  beforeEach(() => {
    settle = {};
    refreshProviderLimits = vi.fn(
      (provider: string, id: string) =>
        new Promise<void>((resolve, reject) => {
          settle[limitsRefreshKey(provider, id)] = { ok: resolve, fail: reject };
        }),
    );
    useAppStore.setState({
      refreshProviderLimits,
      providerAccounts: {
        claude: [
          account("claude", "default"),
          account("claude", "work"),
          account("claude", "old", "signed_out"),
        ],
        codex: [account("codex", "default")],
      },
      providerLimits: {
        claude: {
          default: {
            limits: null,
            refresh: { status: "ok", at: 1, next_allowed_at: null, failures: 0 },
          },
        },
      },
    });
  });

  it("asks every signed-in account: providers side by side, a provider's in turn", async () => {
    const { result } = renderHook(() => useLimitsRefresh());
    let done: Promise<void> = Promise.resolve();
    act(() => {
      done = result.current.refresh(["claude", "codex"]);
    });
    expect(refreshProviderLimits.mock.calls).toEqual([
      ["claude", "default"],
      ["codex", "default"],
    ]);
    expect(result.current.refreshing).toBe(true);

    await act(async () => settle["claude/default"].ok());
    // Never the signed-out one.
    expect(refreshProviderLimits.mock.calls.at(-1)).toEqual(["claude", "work"]);
    expect(result.current.refreshing).toBe(true);

    await act(async () => {
      settle["codex/default"].ok();
      settle["claude/work"].ok();
      await done;
    });
    expect(result.current.refreshing).toBe(false);
    expect(refreshProviderLimits).toHaveBeenCalledTimes(3);
  });

  it("stays busy, and keeps what failed, while an overlapping call is still out", async () => {
    const { result } = renderHook(() => useLimitsRefresh());
    let first: Promise<void> = Promise.resolve();
    let second: Promise<void> = Promise.resolve();
    // A Refresh over Codex alone, then the opening one over an account more.
    act(() => {
      first = result.current.refresh(["codex"]);
    });
    const failCodex = settle["codex/default"].fail;
    act(() => {
      second = result.current.refresh(["claude"], (a) => a.id === "work");
    });

    await act(async () => {
      failCodex("offline");
      await first;
    });
    // The first is done; the second still has a request in flight.
    expect(result.current.refreshing).toBe(true);
    expect(result.current.error).toBe("Terminal login: offline");

    await act(async () => {
      settle["claude/work"].ok();
      await second;
    });
    expect(result.current.refreshing).toBe(false);
    // An account that went fine does not clear another's failure.
    expect(result.current.error).toBe("Terminal login: offline");
  });

  it("drops an account's failure once it is read", async () => {
    const { result } = renderHook(() => useLimitsRefresh());
    let done: Promise<void> = Promise.resolve();
    act(() => {
      done = result.current.refresh(["codex"]);
    });
    await act(async () => {
      settle["codex/default"].fail("offline");
      await done;
    });
    expect(result.current.error).toBe("Terminal login: offline");

    act(() => {
      done = result.current.refresh(["codex"]);
    });
    await act(async () => {
      settle["codex/default"].ok();
      await done;
    });
    expect(result.current.error).toBeNull();
  });

  it("forgets what an account ran into once it is deleted or signed out", async () => {
    const { result } = renderHook(() => useLimitsRefresh());
    const failBoth = async () => {
      let done: Promise<void> = Promise.resolve();
      act(() => {
        done = result.current.refresh(["codex", "claude"], (a) => a.id !== "work");
      });
      await act(async () => {
        settle["codex/default"].fail("offline");
        settle["claude/default"].fail("refused");
        await done;
      });
    };
    await failBoth();
    expect(result.current.error).toBe("Terminal login: offline Terminal login: refused");

    // Codex's account is signed out; Claude's is still there to be refreshed.
    act(() => {
      useAppStore.setState((s) => ({
        providerAccounts: {
          ...s.providerAccounts,
          codex: [account("codex", "default", "signed_out")],
        },
      }));
    });
    expect(result.current.error).toBe("Terminal login: refused");

    // And then deleted outright.
    act(() => {
      useAppStore.setState((s) => ({ providerAccounts: { ...s.providerAccounts, claude: [] } }));
    });
    expect(result.current.error).toBeNull();
  });

  it("on Settings' open asks Codex always and Claude only where nothing was ever read", () => {
    const { result } = renderHook(() => useLimitsRefresh());
    act(() => {
      void result.current.refreshOnOpen(["claude", "codex"]);
    });
    expect(refreshProviderLimits.mock.calls).toEqual([
      ["claude", "work"],
      ["codex", "default"],
    ]);
  });
});
