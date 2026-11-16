// @vitest-environment happy-dom
import { beforeEach, describe, expect, it } from "vitest";
import type { AccountLimits, ProviderAccount } from "@/api/types/providers";
import { useAppStore } from "@/store";
import { render, screen } from "@/test/render";
import { LimitsButton } from "./LimitsButton";

const account = (provider: string, id: string, active = false): ProviderAccount => ({
  provider,
  id,
  managed: id !== "default",
  active,
  status: "signed_in",
  detail: null,
});

// A reset well past the test's clock, so every window still holds.
const later = Math.floor(Date.now() / 1000) + 3_600;
const reading = (fiveHour: number, sevenDay: number): AccountLimits => ({
  limits: {
    five_hour: { percent: fiveHour, resets_at: later },
    seven_day: { percent: sevenDay, resets_at: later },
    as_of: 0,
    source: "stream",
  },
  refresh: null,
});

const chip = () => screen.getByRole("button", { name: /^Usage/ });

describe("LimitsButton", () => {
  beforeEach(() => useAppStore.setState(useAppStore.getInitialState(), true));

  it("is a bare chip until a reading lands", () => {
    render(<LimitsButton />);
    expect(chip().getAttribute("aria-label")).toBe("Usage");
    expect(chip().textContent).toBe("");
    expect(chip().getAttribute("data-tip")).toMatch(/^Usage \(.+\)$/);
  });

  it("carries the weekly allowance used across every listed account", () => {
    useAppStore.setState({
      providerAccounts: {
        claude: [account("claude", "default"), account("claude", "work", true)],
        codex: [account("codex", "default")],
      },
      providerLimits: {
        // A spent five-hour window on one account doesn't move the figure.
        claude: { default: reading(99, 70), work: reading(20, 2) },
        codex: { default: reading(41, 60) },
      },
    });
    render(<LimitsButton />);
    expect(chip().textContent).toBe("44%");
    expect(chip().getAttribute("aria-label")).toBe(
      "Usage · 44% of the weekly allowance used across 3 accounts",
    );
    expect(chip().getAttribute("data-tip")).toMatch(
      /^Usage \(.+\) · 44% of the weekly allowance used across 3 accounts$/,
    );
    expect(chip().className).not.toMatch(/\b(warn|full)\b/);
  });

  it("takes the meters' tint past their warning line", () => {
    useAppStore.setState({
      providerAccounts: { codex: [account("codex", "default")] },
      providerLimits: { codex: { default: reading(12, 96) } },
    });
    render(<LimitsButton />);
    expect(chip().textContent).toBe("96%");
    expect(chip().getAttribute("aria-label")).toBe(
      "Usage · 96% of the weekly allowance used across 1 account",
    );
    expect(chip().className).toMatch(/\bfull\b/);
  });
});
