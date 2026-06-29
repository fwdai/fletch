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

  const onCopy = async () => {
    // Guard the whole call: the Clipboard API is absent in insecure contexts,
    // some webviews, and most test runtimes, and writeText can throw
    // synchronously (not just reject) — which a trailing .catch() would miss.
    try {
      if (!navigator.clipboard?.writeText) return;
      await navigator.clipboard.writeText(text);
    } catch {
      // Denied or unavailable — leave the icon as-is rather than falsely
      // confirming a copy that didn't happen.
      return;
    }
    setCopied(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), 1500);
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
