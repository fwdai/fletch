// @vitest-environment happy-dom
// The workspace-type pill on the new-workspace row: which sandbox a new
// workspace on This Mac runs in. The first test draws the pill's markup the
// way it has always been drawn and compares the two, with a host paired too:
// which machine the workspace runs on is the composer tab's, and pairing one
// changes nothing here.

import { beforeEach, describe, expect, it, vi } from "vitest";

const { availableWorkspaceTypes } = vi.hoisted(() => ({ availableWorkspaceTypes: vi.fn() }));
vi.mock("@/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/api")>();
  return { ...actual, api: { availableWorkspaceTypes } };
});

import type { WorkspaceTypeOption } from "@/api";
import { Icon } from "@/components/Icon";
import { DropdownItem, DropdownMenu, DropdownSection } from "@/components/ui/Dropdown";
import { useAppStore } from "@/store";
import { fireEvent, render, screen } from "@/test/render";
import { WorkspaceTypePicker } from "./WorkspaceTypePicker";

const OPTIONS: WorkspaceTypeOption[] = [
  { kind: "host", available: true, reason: null, default: true },
  { kind: "box", available: false, reason: "VM image not installed", default: false },
];

function picker(onPick = vi.fn()) {
  return render(<WorkspaceTypePicker repoPath="/w/app" provider="claude" onPick={onPick} />);
}

/** The workspace-type pill as it has always been drawn for `OPTIONS`, closed
 *  and open. */
function pillAsBefore(open: boolean): string {
  const drawn = render(
    <span style={{ position: "relative" }}>
      <button
        type="button"
        className="pill is-action tip"
        data-tip="Runs in the macOS sandbox"
        aria-haspopup="menu"
        aria-expanded={open}
      >
        <Icon name="folderLock" />
        <span style={{ color: "var(--fg-2)" }}>in</span>
        <span className="v">macOS sandbox</span>
        <Icon name="chevD" size={9} />
      </button>
      {open && (
        <>
          <div style={{ position: "fixed", inset: 0, zIndex: 199 }} />
          <DropdownMenu className="ws-type-menu" style={{ bottom: "calc(100% + 6px)", left: 0 }}>
            <DropdownSection>Runs in</DropdownSection>
            <DropdownItem
              as="button"
              role="menuitemradio"
              aria-checked={true}
              active={true}
              disabled={false}
            >
              <Icon name="folderLock" size={14} />
              <span className="di-l">macOS sandbox</span>
              <span className="di-m">default</span>
              <Icon name="check" size={13} />
            </DropdownItem>
            <DropdownItem
              as="button"
              role="menuitemradio"
              aria-checked={false}
              active={false}
              disabled={true}
              title="VM image not installed"
            >
              <Icon name="cube" size={14} />
              <span className="di-l">Linux VM</span>
            </DropdownItem>
          </DropdownMenu>
        </>
      )}
    </span>,
  );
  const html = drawn.container.innerHTML;
  drawn.unmount();
  return html;
}

beforeEach(() => {
  vi.clearAllMocks();
  availableWorkspaceTypes.mockResolvedValue(OPTIONS);
  useAppStore.setState(useAppStore.getInitialState(), true);
});

describe("WorkspaceTypePicker", () => {
  it("is the workspace-type pill exactly as before, closed and open", async () => {
    const closed = pillAsBefore(false);
    const opened = pillAsBefore(true);
    const { container } = picker();
    const pill = await screen.findByRole("button", { name: /macOS sandbox/ });

    expect(container.innerHTML).toBe(closed);
    fireEvent.click(pill);
    expect(container.innerHTML).toBe(opened);
    expect(screen.queryByText("This Mac")).toBeNull();
  });

  it("keeps a pick on This Mac", async () => {
    const onPick = vi.fn();
    picker(onPick);
    fireEvent.click(await screen.findByRole("button", { name: /macOS sandbox/ }));
    fireEvent.click(screen.getByRole("menuitemradio", { name: /macOS sandbox/ }));
    expect(onPick).toHaveBeenCalledWith("host");
  });

  it("offers a connected cloud by its provider's name, after the two on this Mac", async () => {
    availableWorkspaceTypes.mockResolvedValue([
      ...OPTIONS,
      { kind: "cloud:fly", label: "Fly.io", available: true, reason: null, default: false },
    ]);
    const onPick = vi.fn();
    const { container } = picker(onPick);
    fireEvent.click(await screen.findByRole("button", { name: /macOS sandbox/ }));

    const rows = screen.getAllByRole("menuitemradio");
    expect(rows.map((r) => r.textContent)).toEqual(["macOS sandboxdefault", "Linux VM", "Fly.io"]);
    expect(container.querySelector(".ws-type-menu .lucide-cloud")).toBeTruthy();
    fireEvent.click(screen.getByRole("menuitemradio", { name: /Fly\.io/ }));
    expect(onPick).toHaveBeenCalledWith("cloud:fly");
  });

  it("draws a picked cloud on the pill, with whose bill it is on", async () => {
    availableWorkspaceTypes.mockResolvedValue([
      ...OPTIONS,
      { kind: "cloud:fly", label: "Fly.io", available: true, reason: null, default: false },
    ]);
    render(
      <WorkspaceTypePicker
        repoPath="/w/app"
        provider="claude"
        current="cloud:fly"
        onPick={vi.fn()}
      />,
    );
    const pill = await screen.findByRole("button", { name: /Fly\.io/ });
    expect(pill.textContent).toBe("inFly.io");
    expect(pill.getAttribute("data-tip")).toBe(
      "Runs on a machine at Fly.io · billed to your Fly.io account",
    );
    expect(pill.querySelector(".lucide-cloud")).toBeTruthy();
  });

  it("is the same pill with a host paired", async () => {
    useAppStore.getState().upsertEnvironment({
      id: "studio",
      name: "Studio",
      kind: "remote",
      connection: "connected",
    });
    const closed = pillAsBefore(false);
    const { container } = picker();
    await screen.findByRole("button", { name: /macOS sandbox/ });
    expect(container.innerHTML).toBe(closed);
  });
});
