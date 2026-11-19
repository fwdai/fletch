import { describe, expect, it } from "vitest";
import { cloudKind, cloudProviderOf, inCloud, isCloudKind } from "./workspaceKind";

describe("workspace kind", () => {
  it("tells a cloud kind from the two on this Mac", () => {
    expect(isCloudKind("cloud:fly")).toBe(true);
    expect(isCloudKind("host")).toBe(false);
    expect(isCloudKind("box")).toBe(false);
  });

  it("reads and writes a cloud kind's provider", () => {
    expect(cloudProviderOf("cloud:fly")).toBe("fly");
    expect(cloudProviderOf("host")).toBeNull();
    expect(cloudProviderOf("box")).toBeNull();
    expect(cloudKind("fly")).toBe("cloud:fly");
    expect(cloudProviderOf(cloudKind("daytona"))).toBe("daytona");
  });

  it("puts a record in the cloud only when it names a provider", () => {
    expect(inCloud({ cloud_provider: "fly" })).toBe(true);
    expect(inCloud({ cloud_provider: null })).toBe(false);
    expect(inCloud({})).toBe(false);
  });
});
