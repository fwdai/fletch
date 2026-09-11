// The installing / failed detail: the installer's own output, verbatim. The
// vendor scripts report no percentages, so their log IS the progress — and on
// failure the exit line they printed is the only thing that tells the user
// what to do next, which is why it's shown instead of a generic apology.

import { type ReactNode, useEffect, useRef } from "react";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { CopyButton } from "@/components/ui/CopyButton";

export function InstallLog({
  command,
  log,
  /** Set once the run failed — the installer's real exit line. */
  error,
  onRetry,
  /** The shared binary-path editor, offered as the manual way out. */
  locate,
}: {
  command?: string;
  log: string[];
  error?: string;
  onRetry: () => void;
  locate: ReactNode;
}) {
  const box = useRef<HTMLDivElement>(null);

  // The backend's first "running" line is the `$ command` itself, so the log
  // already reads like a terminal; this only covers the gap before it lands.
  const lines = log.length > 0 ? log : command ? [`$ ${command}`] : [];
  const text = error ? [...lines, error].join("\n") : lines.join("\n");

  // Follow the tail as output streams in — `text` is what changes per line.
  useEffect(() => {
    const el = box.current;
    if (el && text) el.scrollTop = el.scrollHeight;
  }, [text]);

  return (
    <>
      <div className="set-prov-log mono text-xs" ref={box}>
        <pre>{lines.join("\n")}</pre>
        {error && <pre className="err">{error}</pre>}
      </div>
      {error && (
        <div className="set-prov-detail-actions flex-center">
          <Button variant="outline" size="sm" onClick={onRetry}>
            <Icon name="refresh" size={12} />
            Retry
          </Button>
          <CopyButton text={text} tip="Copy log" />
        </div>
      )}
      {error && locate}
    </>
  );
}
