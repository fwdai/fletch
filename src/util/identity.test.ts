import { describe, expect, it } from "vitest";
import { displayIdentity } from "./identity";

const profile = (p: Partial<{ firstName: string; lastName: string; email: string }>) => ({
  id: "a",
  firstName: "",
  lastName: "",
  email: "",
  avatarUrl: null,
  ...p,
});

describe("displayIdentity", () => {
  it("prefers the saved profile over git, even a partial one", () => {
    const id = displayIdentity(profile({ email: "ada@example.com" }), {
      name: "Grace Hopper",
      email: "grace@navy.mil",
    });
    expect(id).toEqual({
      name: "",
      email: "ada@example.com",
      initials: "A",
      source: "profile",
    });
  });

  it("falls back to the global git identity, splitting its name for initials", () => {
    const id = displayIdentity(profile({}), { name: "Grace Brewster Hopper", email: "g@h.io" });
    expect(id).toEqual({
      name: "Grace Brewster Hopper",
      email: "g@h.io",
      initials: "GB",
      source: "git",
    });
  });

  it("uses a git email alone when that is all git knows", () => {
    expect(displayIdentity(null, { name: null, email: "g@h.io" })).toEqual({
      name: "",
      email: "g@h.io",
      initials: "G",
      source: "git",
    });
  });

  it("reports nothing rather than a placeholder when nothing is known", () => {
    expect(displayIdentity(null, null)).toEqual({
      name: "",
      email: "",
      initials: "",
      source: "none",
    });
    expect(displayIdentity(profile({ firstName: "  " }), { name: " ", email: null }).source).toBe(
      "none",
    );
  });
});
