import { describe, expect, it } from "vitest";
import { tipFor } from "./index";

const base = {
  dictationAvailable: true,
  autoStops: true,
  error: null,
  sendLabel: "Send",
};

describe("tipFor", () => {
  it("promises the pause while listening only when the setting is on", () => {
    expect(tipFor("listening", base)).toContain("stops when you pause");
  });

  it("offers the manual stop when the setting is off", () => {
    const tip = tipFor("listening", { ...base, autoStops: false });
    expect(tip).toContain("Stop & transcribe");
    expect(tip).not.toContain("stops when you pause");
  });
});
