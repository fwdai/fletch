// @vitest-environment happy-dom
import { useRef } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { useAppStore } from "@/store";
import { fireEvent, render, renderHook } from "@/test/render";
import { useGlobalShortcuts } from "@/util/shortcuts";
import { useFocusScope } from "./useFocusScope";

function Dialog({ autoFocus, label = "dialog" }: { autoFocus?: boolean; label?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useFocusScope(ref);
  return (
    <div ref={ref} role="dialog" aria-label={label}>
      <button type="button">first</button>
      <input aria-label="field" autoFocus={autoFocus} />
      <button type="button">last</button>
    </div>
  );
}

/** A field outside the dialog, focused, standing in for the composer. */
function focusedOutside(): HTMLInputElement {
  const outside = document.createElement("input");
  document.body.append(outside);
  outside.focus();
  return outside;
}

const tab = (el: Element, shiftKey = false) => fireEvent.keyDown(el, { key: "Tab", shiftKey });

/** Past the microtask the focus check waits in. */
const settled = () => Promise.resolve();

describe("useFocusScope", () => {
  afterEach(() => {
    document.body.replaceChildren();
  });

  it("moves focus onto the dialog as it opens, and back when it closes", () => {
    const outside = focusedOutside();
    const { getByRole, unmount } = render(<Dialog />);
    expect(document.activeElement).toBe(getByRole("dialog"));
    unmount();
    expect(document.activeElement).toBe(outside);
  });

  it("leaves focus on a field the dialog focused itself", () => {
    focusedOutside();
    const { getByLabelText } = render(<Dialog autoFocus />);
    expect(document.activeElement).toBe(getByLabelText("field"));
  });

  it("wraps Tab at both ends", () => {
    const { getByText, getByRole } = render(<Dialog />);
    const first = getByText("first");
    const last = getByText("last");

    last.focus();
    tab(last);
    expect(document.activeElement).toBe(first);

    tab(first, true);
    expect(document.activeElement).toBe(last);

    // From the dialog itself, backwards is the last control too.
    const dialog = getByRole("dialog");
    dialog.focus();
    tab(dialog, true);
    expect(document.activeElement).toBe(last);
  });

  it("does not take focus back from whatever claimed it before the close", () => {
    const outside = focusedOutside();
    const { unmount } = render(<Dialog />);
    const elsewhere = document.createElement("textarea");
    document.body.append(elsewhere);
    elsewhere.focus();
    unmount();
    expect(document.activeElement).toBe(elsewhere);
    expect(document.activeElement).not.toBe(outside);
  });

  it("takes focus back from anything outside while it is open", async () => {
    const outside = focusedOutside();
    const { getByText } = render(<Dialog />);
    const first = getByText("first");
    first.focus();
    await settled();

    outside.focus();
    await settled();
    // Back to where it was inside, not to the top of the dialog.
    expect(document.activeElement).toBe(first);
  });

  it("leaves focus alone inside a dialog opened over it", async () => {
    const { getByRole } = render(
      <>
        <Dialog label="under" />
        <Dialog label="over" autoFocus />
      </>,
    );
    await settled();
    const over = getByRole("dialog", { name: "over" });
    expect(over.contains(document.activeElement)).toBe(true);

    // The one underneath does not answer Tab while it is covered.
    const last = over.querySelector("button:last-of-type") as HTMLElement;
    last.focus();
    tab(last);
    expect(over.contains(document.activeElement)).toBe(true);
  });

  it("keeps the keyboard when a global shortcut focuses the composer behind it", async () => {
    const composer = document.createElement("textarea");
    composer.className = "composer-input";
    document.body.append(composer);
    useAppStore.setState({ paletteOpen: false });
    renderHook(() => useGlobalShortcuts());
    const { getByRole } = render(<Dialog />);
    const dialog = getByRole("dialog");

    // ⌘L / Ctrl+L: "Focus the composer".
    fireEvent.keyDown(dialog, { key: "l", metaKey: true });
    fireEvent.keyDown(dialog, { key: "l", ctrlKey: true });
    await settled();
    expect(dialog.contains(document.activeElement)).toBe(true);

    // And Tab stays inside rather than walking the page behind.
    tab(document.activeElement as Element, true);
    expect(dialog.contains(document.activeElement)).toBe(true);
  });
});
