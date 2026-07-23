import { useEffect, useRef, useState } from "react";
import { issueDisplayKey, type TrackerIssue } from "@/api";
import { Icon } from "@/components/Icon";
import { DropdownItem, DropdownMenu, DropdownSection } from "@/components/ui/Dropdown";
import { IconButton } from "@/components/ui/IconButton";
import { Scrim } from "@/components/ui/Scrim";

/** How long a fetched issue list stays fresh before the next open refetches —
 *  mirrors the "#" PR source's cache rationale (absorb rapid open/close
 *  cycles, still pick up new issues within a session). */
const ISSUE_CACHE_MS = 15_000;

const MAX_HEIGHT = 34 * 8; // ~8 dd-item rows before the list scrolls

interface Props {
  /** Lists open issues across the connected tracker sources (GitHub, Linear).
   *  Called on menu open, cached briefly. */
  listIssues: () => Promise<TrackerIssue[]>;
  /** Fired with the picked issue — the composer inserts the brief; parents
   *  additionally tag the draft (`issueRef`) where one exists. */
  onPick: (issue: TrackerIssue) => void;
}

/** Footer dropdown for attaching a tracker issue to the prompt: pick an open
 *  GitHub issue / Linear ticket and the composer is seeded with its brief
 *  (title + body + url + branch suggestion), so the agent works the exact
 *  issue. Source-agnostic — it renders whatever `listIssues` returns. */
export function IssuePicker({ listIssues, onPick }: Props) {
  const [open, setOpen] = useState(false);
  const [issues, setIssues] = useState<TrackerIssue[] | null>(null);

  // Fetch on open with a short cache window; refs so an inline `listIssues`
  // prop and the timestamp don't refire the effect (same pattern as the "#"
  // PR autocomplete source).
  const ref = useRef(listIssues);
  ref.current = listIssues;
  const lastFetch = useRef(0);
  useEffect(() => {
    if (!open) return;
    if (Date.now() - lastFetch.current < ISSUE_CACHE_MS) return;
    lastFetch.current = Date.now();
    let alive = true;
    ref
      .current()
      .then((list) => {
        if (alive) setIssues(list);
      })
      .catch(() => {
        if (alive) setIssues([]);
      });
    return () => {
      alive = false;
    };
  }, [open]);

  return (
    <span style={{ position: "relative", minWidth: 0 }}>
      <IconButton
        className="composer-action"
        tip="Work an issue"
        active={open}
        onClick={() => setOpen((v) => !v)}
      >
        <Icon name="issue" size={15} />
      </IconButton>

      {open && (
        <>
          <Scrim onClose={() => setOpen(false)} />
          <DropdownMenu
            // Right-anchored: the trigger now sits at the composer's right
            // edge, so a left-anchored 280px+ menu would overflow the frame.
            style={{
              bottom: "calc(100% + 6px)",
              right: 0,
              padding: 0,
              overflow: "hidden",
              minWidth: 280,
              // Long issue titles must not stretch the menu — cap it and let
              // each row's title ellipsize (`.di-l`).
              maxWidth: 420,
            }}
          >
            <DropdownSection>Open issues</DropdownSection>
            <div style={{ maxHeight: MAX_HEIGHT, overflowY: "auto" }}>
              {issues === null ? (
                <DropdownItem disabled style={{ padding: "7px 9px" }}>
                  Loading…
                </DropdownItem>
              ) : issues.length === 0 ? (
                <DropdownItem disabled style={{ padding: "7px 9px" }}>
                  No open issues
                </DropdownItem>
              ) : (
                issues.map((issue) => (
                  <DropdownItem
                    key={`${issue.source}:${issue.key}`}
                    as="button"
                    style={{ padding: "7px 9px", width: "100%" }}
                    title={issue.title}
                    onClick={() => {
                      onPick(issue);
                      setOpen(false);
                    }}
                  >
                    {/* Fixed mono key on the left, then the title filling the
                        row (`.di-l` is flex:1 + ellipsis) so rows read
                        left-aligned and clip instead of widening the menu. */}
                    <span className="di-m" style={{ flexShrink: 0 }}>
                      {issueDisplayKey(issue)}
                    </span>
                    <span className="di-l" style={{ textAlign: "left" }}>
                      {issue.title}
                    </span>
                  </DropdownItem>
                ))
              )}
            </div>
          </DropdownMenu>
        </>
      )}
    </span>
  );
}
