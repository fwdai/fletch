// MissionControl/IssueInbox.tsx — the Home issue inbox: a quiet section below
// the review queue listing open issues for the workspace's tracked repos,
// from every connected tracker (GitHub issues, Linear tickets). "Start work"
// lands in the new-task composer, fully prefilled. This file is the data/poll
// shell; row rendering lives in IssueRow, the pure derivation in inbox.ts.

import { useCallback, useMemo, useState } from "react";
import { api, type TrackerIssue } from "@/api";
import { Icon } from "@/components/Icon";
import { getLinearTeamId } from "@/storage/projectSettings";
import { useAppStore } from "@/store";
import { usePoll } from "@/util/hooks";
import { IssueRow } from "./IssueRow";
import { deriveInboxRows, type InboxRepo } from "./inbox";

/** Slow, connection-gated cadence — the inbox is secondary; open issues
 *  change on human timescales, so a modest poll matches the existing
 *  PR-state cadence. */
const POLL_MS = 120_000;

export function IssueInbox() {
  const githubConnected = useAppStore((s) => s.github?.authenticated ?? false);
  const linearConnected = useAppStore((s) => s.linear?.authenticated ?? false);
  const repoPaths = useAppStore((s) => s.workspace?.repos ?? []);
  const projects = useAppStore((s) => s.workspace?.projects ?? []);
  const startWorkFromIssue = useAppStore((s) => s.startWorkFromIssue);

  const anyConnected = githubConnected || linearConnected;

  // Open issues keyed by repo path. Sources degrade quietly inside the
  // backend (no token / non-GitHub origin / no Linear team / rate-limit
  // pause all read as "no issues") — the section just hides.
  const [byRepo, setByRepo] = useState<Record<string, TrackerIssue[]>>({});

  const poll = useCallback(async () => {
    if (!anyConnected || repoPaths.length === 0) return;
    const entries = await Promise.all(
      repoPaths.map(async (path) => {
        const projectId = projects.find((r) => r.path === path)?.project_id ?? "";
        const teamId = await getLinearTeamId(projectId).catch(() => undefined);
        const issues = await api.listTrackerIssues(path, teamId).catch(() => []);
        return [path, issues] as const;
      }),
    );
    setByRepo(Object.fromEntries(entries));
  }, [anyConnected, repoPaths, projects]);

  usePoll(poll, POLL_MS, [poll]);

  // Repo display label: the project's user label/name, else the folder name.
  const labelFor = useCallback(
    (path: string) => {
      const ref = projects.find((r) => r.path === path);
      return ref?.label || ref?.name || path.split("/").filter(Boolean).pop() || path;
    },
    [projects],
  );

  const rows = useMemo(() => {
    const repos: InboxRepo[] = repoPaths
      .map((path) => ({ repoPath: path, repoLabel: labelFor(path), issues: byRepo[path] ?? [] }))
      .filter((r) => r.issues.length > 0);
    return deriveInboxRows(repos);
  }, [repoPaths, byRepo, labelFor]);

  const multiRepo = useMemo(() => new Set(rows.map((r) => r.repoPath)).size > 1, [rows]);

  // Quiet degradation: no connected tracker, no tracked repos, or no open
  // issues → the section disappears entirely. Never an error, never a parked
  // spinner.
  if (!anyConnected || rows.length === 0) return null;

  return (
    <div className="mc-inbox-wrap">
      <div className="mc-inbox-head">
        <Icon name="inbox" size={13} />
        <span>Open issues</span>
        <span className="mc-inbox-count">{rows.length}</span>
      </div>
      <div className="mc-inbox-list">
        {rows.map((row) => (
          <IssueRow
            key={row.key}
            row={row}
            showRepo={multiRepo}
            onStart={() => void startWorkFromIssue(row.repoPath, row.issue)}
          />
        ))}
      </div>
    </div>
  );
}
