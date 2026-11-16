import { describe, expect, it } from "vitest";
import { roadmapItemBrief } from "./roadmapBrief";

describe("roadmapItemBrief", () => {
  it("names the item, says why, and lists what done means", () => {
    expect(
      roadmapItemBrief({
        code: "FLT-7",
        title: "Export invoices",
        why: "Accounting asks for it monthly.",
        accept: ["CSV download", "Totals match"],
      }),
    ).toBe(
      "FLT-7: Export invoices\n\nAccounting asks for it monthly.\n\nDone when:\n- CSV download\n- Totals match",
    );
  });

  it("leaves the checklist out when there are no criteria", () => {
    expect(roadmapItemBrief({ code: "FLT-8", title: "Tidy", why: "", accept: [] })).toBe(
      "FLT-8: Tidy",
    );
  });
});
