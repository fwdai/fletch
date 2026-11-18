// @vitest-environment happy-dom
// A blocked send is refused in words by the composer, so the arrow has to let
// the attempt through: a disabled button swallows the click and says nothing.
import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@/test/render";
import { PrimaryControl } from ".";

describe("PrimaryControl with a blocked send", () => {
  it("passes the click on, and says the arrow is unavailable", () => {
    const onSend = vi.fn();
    render(
      <PrimaryControl
        state="draft"
        dictationAvailable={false}
        sendBlocked="Not installed on Studio"
        onSend={onSend}
      />,
    );
    const send = screen.getByRole("button", { name: "Send" }) as HTMLButtonElement;
    expect(send.disabled).toBe(false);
    expect(send.getAttribute("aria-disabled")).toBe("true");
    fireEvent.click(send);
    expect(onSend).toHaveBeenCalledTimes(1);
  });

  it("stays inert while the composer takes no input", () => {
    const onSend = vi.fn();
    render(
      <PrimaryControl
        state="draft"
        dictationAvailable={false}
        sendBlocked="Not installed on Studio"
        disabled
        onSend={onSend}
      />,
    );
    const send = screen.getByRole("button", { name: "Send" }) as HTMLButtonElement;
    expect(send.disabled).toBe(true);
    fireEvent.click(send);
    expect(onSend).not.toHaveBeenCalled();
  });
});
