// MissionControl/IssueInbox.tsx — the Home issue inbox: a quiet section below
// the review queue listing open issues for the workspace's tracked repos,
// from every connected tracker (GitHub issues, Linear tickets). "Start work"
// lands in the new-task composer, fully prefilled; "Add to roadmap" routes the
// issue onto the project's board as a ghost to rule on later. This file is the
// data/poll shell; row rendering lives in IssueRow, the pure derivations in
// inbox.ts and funnel.ts.

import { useCallback, useMemo, useState } from "react";
import { api, type TrackerIssue } from "@/api";
import { Icon } from "@/components/Icon";
import { getLinearTeamId } from "@/storage/projectSettings";
import { useAppStore } from "@/store";
import { usePoll } from "@/util/hooks";
import { IssueRow } from "./IssueRow";
import { deriveInboxRows, type InboxRepo } from "./inbox";
import { useIssueFunnel } from "./useIssueFunnel";

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

  // Which board an issue's repo routes onto. Undefined is a legitimate answer
  // (a pinned repo that isn't part of a project yet) and means no roadmap
  // action at all.
  const projectIdFor = useCallback(
    (path: string) => projects.find((r) => r.path === path)?.project_id || undefined,
    [projects],
  );

  // Only the boards actually reachable from the listed issues are read.
  const projectIds = useMemo(() => {
    const ids = new Set<string>();
    for (const row of rows) {
      const id = projectIdFor(row.repoPath);
      if (id) ids.add(id);
    }
    return [...ids];
  }, [rows, projectIdFor]);

  const funnel = useIssueFunnel(projectIds);
  const [funnelError, setFunnelError] = useState<string | null>(null);

  const addToRoadmap = useCallback(
    async (projectId: string, issue: TrackerIssue) => {
      setFunnelError(null);
      try {
        await funnel.route(projectId, issue);
      } catch (e) {
        setFunnelError(e instanceof Error ? e.message : String(e));
      }
    },
    [funnel],
  );

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
      {funnelError && <div className="mc-inbox-error">{funnelError}</div>}
      <div className="mc-inbox-list">
        {rows.map((row) => {
          const action = funnel.actionFor(projectIdFor(row.repoPath), row.issue);
          return (
            <IssueRow
              key={row.key}
              row={row}
              showRepo={multiRepo}
              onStart={() => void startWorkFromIssue(row.repoPath, row.issue)}
              funnel={action}
              onAddToRoadmap={() => {
                if (action.kind === "add") void addToRoadmap(action.projectId, row.issue);
              }}
            />
          );
        })}
      </div>
    </div>
  );
}
