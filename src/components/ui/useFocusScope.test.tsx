// @vitest-environment happy-dom
import { useRef } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { fireEvent, render } from "@/test/render";
import { useFocusScope } from "./useFocusScope";

function Dialog({ autoFocus }: { autoFocus?: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  useFocusScope(ref);
  return (
    <div ref={ref} role="dialog">
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
});
