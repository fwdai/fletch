import { useEffect, useRef, useState } from "react";

/** Copy `text` to the clipboard. `copied` holds true briefly after a copy that
 *  really happened, so the caller can confirm it. */
export function useCopy(text: string): { copied: boolean; copy: () => Promise<void> } {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>();

  // Clear the pending revert if the caller unmounts mid-confirmation.
  useEffect(() => () => clearTimeout(timer.current), []);

  const copy = async () => {
    // Guard the whole call: the Clipboard API is absent in insecure contexts,
    // some webviews, and most test runtimes, and writeText can throw
    // synchronously (not just reject) — which a trailing .catch() would miss.
    try {
      if (!navigator.clipboard?.writeText) return;
      await navigator.clipboard.writeText(text);
    } catch {
      // Denied or unavailable — leave the state as-is rather than falsely
      // confirming a copy that didn't happen.
      return;
    }
    setCopied(true);
    clearTimeout(timer.current);
    timer.current = setTimeout(() => setCopied(false), 1500);
  };

  return { copied, copy };
}
