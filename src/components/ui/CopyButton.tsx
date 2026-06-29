import { useEffect, useRef, useState } from "react";
import { Icon } from "../Icon";
import { IconButton } from "./IconButton";

/** Copies `text` to the clipboard and briefly swaps the copy icon for a check
 *  as confirmation. Used under chat messages; generic enough for any "copy
 *  this string" affordance. */
export function CopyButton({
  text,
  tip = "Copy",
  className,
}: {
  text: string;
  tip?: string;
  className?: string;
}) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>();

  // Clear the pending revert if the button unmounts mid-confirmation.
  useEffect(() => () => clearTimeout(timer.current), []);

  const onCopy = () => {
    navigator.clipboard
      ?.writeText(text)
      .then(() => {
        setCopied(true);
        clearTimeout(timer.current);
        timer.current = setTimeout(() => setCopied(false), 1500);
      })
      .catch(() => {});
  };

  return (
    <IconButton
      size="xs"
      tip={copied ? "Copied" : tip}
      className={className}
      onClick={onCopy}
      aria-label="Copy message"
    >
      <Icon name={copied ? "check" : "copy"} />
    </IconButton>
  );
}
