// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, type Mock, vi } from "vitest";
import type { ProviderAccount } from "@/api/types/providers";
import { useAppStore } from "@/store";
import { act, renderHook } from "@/test/render";
import { limitsKey, useLimitsRefresh } from "./useLimitsRefresh";

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
  /** Each request waits here until the test lets it through. */
  let release: Record<string, () => void>;
  let refreshProviderLimits: Mock<(provider: string, account: string) => Promise<void>>;

  beforeEach(() => {
    release = {};
    refreshProviderLimits = vi.fn(
      (provider: string, id: string) =>
        new Promise<void>((resolve) => {
          release[limitsKey(provider, id)] = resolve;
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

  it("asks every signed-in account, and names the ones still waiting", async () => {
    const { result } = renderHook(() => useLimitsRefresh());
    let done: Promise<void> = Promise.resolve();
    act(() => {
      done = result.current.refresh(["claude", "codex"]);
    });
    // Providers side by side, a provider's accounts in turn; never a signed-out one.
    expect(refreshProviderLimits.mock.calls).toEqual([
      ["claude", "default"],
      ["codex", "default"],
    ]);
    expect([...result.current.pending].sort()).toEqual([
      "claude/default",
      "claude/work",
      "codex/default",
    ]);
    expect(result.current.refreshing).toBe(true);

    await act(async () => release["claude/default"]());
    expect(result.current.pending.has("claude/default")).toBe(false);
    expect(result.current.pending.has("claude/work")).toBe(true);

    await act(async () => {
      release["codex/default"]();
      release["claude/work"]();
      await done;
    });
    expect(result.current.refreshing).toBe(false);
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
