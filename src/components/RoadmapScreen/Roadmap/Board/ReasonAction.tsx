// Board/ReasonAction.tsx — a secondary card action whose command requires a
// reason, asked for in place.
//
// Extracted from HoldControl's HoldAction when Reject needed the same gesture:
// both commands refuse a blank reason backend-side, so it is asked for here
// rather than sent blank and bounced. The prompt reuses the strip's inline
// pattern — an input that appears exactly where the button was, Enter to
// commit, Escape to back out — so writing a reason feels like answering a
// question inline, not like opening a form. Committing with an empty box
// simply does nothing, which is the same answer as Cancel and needs no error
// state to say so.

import { type KeyboardEvent, useState } from "react";
import { Icon, type IconName } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { TextInput } from "@/components/ui/TextInput";

export function ReasonAction({
  icon,
  label,
  commitLabel = label,
  tip,
  placeholder,
  onCommit,
}: {
  icon: IconName;
  /** The idle button's text ("Hold", "Reject…"). */
  label: string;
  /** The commit button's text, when the ellipsis convention makes the idle
   *  label wrong for it. Defaults to `label`. */
  commitLabel?: string;
  tip: string;
  /** The question the input asks — each command owes a different one. */
  placeholder: string;
  /** Called with the trimmed, non-blank reason. */
  onCommit: (reason: string) => void;
}) {
  const [reason, setReason] = useState<string | null>(null);

  if (reason == null) {
    return (
      <Button variant="ghost" size="sm" tip={tip} onClick={() => setReason("")}>
        <Icon name={icon} size={11} /> {label}
      </Button>
    );
  }

  const commit = () => {
    const text = reason.trim();
    if (text) onCommit(text);
    setReason(null);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") commit();
    if (e.key === "Escape") setReason(null);
  };

  return (
    <span className="rm-hold-ask flex-center">
      <TextInput
        // The input appears exactly where the button the user just pressed was,
        // so the caret belongs in it — otherwise the gesture is click, then click
        // again, to type the thing they opened the box to type.
        autoFocus
        className="rm-hold-input"
        placeholder={placeholder}
        value={reason}
        onChange={(e) => setReason(e.target.value)}
        onKeyDown={onKeyDown}
      />
      <Button variant="ghost" size="sm" onClick={() => setReason(null)}>
        Cancel
      </Button>
      <Button variant="primary" size="sm" onClick={commit}>
        <Icon name={icon} size={11} /> {commitLabel}
      </Button>
    </span>
  );
}
