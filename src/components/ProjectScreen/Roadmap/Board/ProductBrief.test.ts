import { describe, expect, it } from "vitest";
import { briefAskLabel } from "./ProductBrief";

describe("briefAskLabel", () => {
  it("distinguishes the first brief from a revision", () => {
    // The two asks are different decisions: one admits a document the project
    // has never had, the other replaces one the user already agreed to — and
    // "new product brief" over an empty tab would read as a diff against
    // nothing.
    expect(briefAskLabel(false)).toBe("PM drafted the first product brief");
    expect(briefAskLabel(true)).toBe("PM proposes a new product brief");
  });
});
