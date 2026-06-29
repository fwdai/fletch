import { useState } from "react";
import { Icon } from "../Icon";
import { IconButton } from "./IconButton";

/** Re-runs a failed user turn. Shown in the action row under the last user
 *  message when the agent's response errored out (connection drop, auth, a
 *  crashed process). Mirrors CopyButton's placement; the actual resume/resend
 *  lives in the `retryUserMessage` store action. */
export function RetryButton({
  onRetry,
  tip = "Retry",
  className,
}: {
  onRetry: () => void | Promise<void>;
  tip?: string;
  className?: string;
}) {
  const [retrying, setRetrying] = useState(false);
  return (
    <IconButton
      size="xs"
      tip={retrying ? "Retrying…" : tip}
      className={className}
      disabled={retrying}
      onClick={() => {
        // Guard against a double-click firing two sends: the button stays
        // mounted (status is still "error") until the resume/resend lands.
        setRetrying(true);
        void Promise.resolve(onRetry()).finally(() => setRetrying(false));
      }}
      aria-label="Retry message"
    >
      <Icon name="refresh" />
    </IconButton>
  );
}
