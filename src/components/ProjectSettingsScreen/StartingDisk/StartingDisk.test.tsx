// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, type MockInstance, vi } from "vitest";
import {
  api,
  type ProjectFileChangedEvent,
  type ProjectFileReport,
  type ProjectFileSeed,
  type ProjectFileStatus,
} from "@/api";
import { useAppStore } from "@/store";
import { act, fireEvent, render, screen } from "@/test/render";
import { FILE_LABEL } from "../projectFile/files";
import { StartingDisk } from ".";

const changed = vi.hoisted(() => ({ emit: (_: { project_id: string }) => {} }));
vi.mock("@/api", async (original) => ({
  ...(await original<typeof import("@/api")>()),
  onProjectFileChanged: (cb: (e: ProjectFileChangedEvent) => void) => {
    changed.emit = cb;
    return Promise.resolve(() => {});
  },
}));

const COMMENTS =
  ".fletch/settings.json: this file has comments, which an edit from Fletch would drop; edit it by hand instead";

const report = (
  which: "shared" | "local",
  seed: Partial<ProjectFileSeed> = {},
  rest: Partial<ProjectFileReport> = {},
): ProjectFileReport => ({
  path: FILE_LABEL[which],
  exists: true,
  error: null,
  warnings: [],
  allow: [],
  disable: [],
  seed: { enabled: null, exclude: [], warnings: [], ...seed },
  ...rest,
});

const statusOf = (files: ProjectFileReport[], rest: Partial<ProjectFileStatus> = {}) => {
  const enabled = files.reduce<boolean>((on, f) => f.seed.enabled ?? on, true);
  const exclude = [...new Set(files.flatMap((f) => f.seed.exclude))].sort();
  return {
    project_id: "p1",
    repo_path: "/repos/acme",
    error: null,
    files,
    egress: { applied: [], pending: [], disabled: [] },
    seed: { enabled, exclude },
    ...rest,
  } satisfies ProjectFileStatus;
};

describe("StartingDisk", () => {
  let edit: MockInstance<typeof api.projectFileEdit>;

  beforeEach(() => {
    useAppStore.setState(useAppStore.getInitialState(), true);
    edit = vi.spyOn(api, "projectFileEdit").mockImplementation(async () => statusOf([]));
  });

  const mount = async (status: ProjectFileStatus) => {
    vi.spyOn(api, "projectFileStatus").mockResolvedValue(status);
    render(<StartingDisk projectId="p1" />);
    return screen.findByRole("switch");
  };

  it("shows the setting, each folder left out with its file, and the section's warnings", async () => {
    const toggle = await mount(
      statusOf([
        report(
          "shared",
          {
            exclude: ["uploads"],
            warnings: ["`seed.exclude`: `../x` is not a path inside the checkout"],
          },
          { warnings: ["unknown setting `run`"] },
        ),
        report("local", { enabled: false, exclude: ["data/db"] }),
      ]),
    );
    expect(toggle.getAttribute("aria-checked")).toBe("false");
    expect(screen.getByText("Off in .fletch/settings.local.json.")).toBeTruthy();
    expect(screen.getByText("uploads")).toBeTruthy();
    expect(screen.getByText("data/db")).toBeTruthy();
    expect(screen.getByText(/is not a path inside the checkout/)).toBeTruthy();
    expect(screen.queryByText(/unknown setting/)).toBeNull();
    // A file's row is there only while it has something to say of the disk.
    expect(screen.getByText(FILE_LABEL.shared, { selector: ".mono" })).toBeTruthy();
    expect(screen.queryByText(FILE_LABEL.local, { selector: ".mono" })).toBeNull();
  });

  it("turns the disk off in the team's file when the team keeps one", async () => {
    fireEvent.click(await mount(statusOf([report("shared")])));
    await vi.waitFor(() =>
      expect(edit).toHaveBeenCalledWith("p1", "shared", { seedEnabled: false }),
    );
  });

  it("adds a folder to the file picked beside it, and takes one out of the file that holds it", async () => {
    await mount(statusOf([report("shared", { exclude: ["uploads"] })]));
    fireEvent.click(screen.getByRole("button", { name: "Just me" }));
    const input = screen.getByRole("textbox", { name: "Folder to leave out" });
    fireEvent.change(input, { target: { value: " certs " } });
    fireEvent.keyDown(input, { key: "Enter" });
    await vi.waitFor(() =>
      expect(edit).toHaveBeenCalledWith("p1", "local", { seedExcludeAdd: ["certs"] }),
    );
    await vi.waitFor(() => expect(input).toHaveProperty("value", ""));

    fireEvent.click(screen.getByRole("button", { name: "Remove uploads" }));
    await vi.waitFor(() =>
      expect(edit).toHaveBeenCalledWith("p1", "shared", { seedExcludeRemove: ["uploads"] }),
    );
  });

  it("says why the host refused an edit and keeps the draft", async () => {
    edit.mockRejectedValue(COMMENTS);
    await mount(statusOf([report("shared")]));
    const input = screen.getByRole("textbox", { name: "Folder to leave out" });
    fireEvent.change(input, { target: { value: "uploads" } });
    fireEvent.click(screen.getByRole("button", { name: "Add" }));
    expect(await screen.findByText(COMMENTS)).toBeTruthy();
    expect(screen.getByText("Couldn't save")).toBeTruthy();
    expect(input).toHaveProperty("value", "uploads");
  });

  it("offers no edit when the repository could not be read", async () => {
    const toggle = await mount(statusOf([], { error: "/w/acme is inside an agent's workspace" }));
    expect(toggle).toHaveProperty("disabled", true);
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.getByText("/w/acme is inside an agent's workspace")).toBeTruthy();
  });

  it("reads the files again when the host says they changed", async () => {
    const toggle = await mount(statusOf([report("shared")]));
    vi.spyOn(api, "projectFileStatus").mockResolvedValue(
      statusOf([report("shared", { enabled: false })]),
    );
    act(() => changed.emit({ project_id: "p1" }));
    await vi.waitFor(() => expect(toggle.getAttribute("aria-checked")).toBe("false"));
    expect(screen.getByText("Off in .fletch/settings.json.")).toBeTruthy();
  });
});
