import { Icon } from "@desktop/components/Icon";
import { useEffect, useRef, useState } from "react";
import { Nav, ProviderMark, Segmented } from "../../components/ui";
import { baseOf, branchOf, isBusy, STATUS_LABEL } from "../../lib/agents";
import { fmtElapsed, useElapsed } from "../../lib/hooks";
import { subagentCounts, subagentLabel } from "../../lib/subagents";
import { agentOf, projectOf, useStore } from "../../store";
import { ChangesTab } from "./ChangesTab";
import { ChatTab } from "./ChatTab";
import { CodeTab } from "./CodeTab";
import { Composer } from "./Composer";

const TABS = ["chat", "code", "changes"] as const;
type Tab = (typeof TABS)[number];

function SpawnPane({ task, base, branch }: { task: string; base: string; branch: string }) {
  return (
    <div className="scroll">
      <div className="spawn">
        {task && <div className="prompt rise">{task}</div>}
        <div className="steps rise">
          {[`Creating worktree from ${base}`, `Checking out ${branch}`, "Booting the agent"].map(
            (s) => (
              <div key={s} className="step on">
                <span className="ck" />
                {s}
              </div>
            ),
          )}
        </div>
      </div>
    </div>
  );
}

function Subtitle({ agentId }: { agentId: string }) {
  const agent = useStore((s) => agentOf(s, agentId));
  const startedAt = useStore((s) => s.turnStartedAt[agentId]);
  const pending = useStore((s) => Object.keys(s.pendingToolUse[agentId] ?? {}).length);
  const git = useStore((s) => s.gitStates[agentId]);
  const pr = useStore((s) => s.prStates[agentId]);
  const tasks = useStore((s) => s.backgroundTasks[agentId]);
  const elapsed = useElapsed(startedAt, !!agent && isBusy(agent));
  if (!agent) return null;
  if (pending > 0) {
    return (
      <>
        <span className="dot waiting" style={{ width: 6, height: 6 }} />
        Needs approval
      </>
    );
  }
  if (agent.status === "spawning" || agent.status === "running") {
    return (
      <>
        <span className={`dot ${agent.status}`} style={{ width: 6, height: 6 }} />
        {STATUS_LABEL[agent.status]}
        {startedAt ? ` · ${fmtElapsed(elapsed)}` : ""}
      </>
    );
  }
  if (agent.status === "error") {
    return (
      <>
        <span className="dot error" style={{ width: 6, height: 6 }} />
        Paused · error
      </>
    );
  }
  // The turn is over but its sub-agents are not: the header keeps saying so
  // rather than dropping to the diff line.
  const subagents = subagentCounts(tasks, Date.now());
  if (subagents.running > 0) {
    return (
      <>
        <span className="dot running" style={{ width: 6, height: 6 }} />
        {subagentLabel(subagents.running)} working
      </>
    );
  }
  // A planning chat has no checkout to report on: nothing loads its git state
  // (see `loadAgent`), so the diff line below would read "clean" forever.
  if (agent.purpose) return <>Planning chat</>;
  const changed = (git?.additions ?? 0) + (git?.deletions ?? 0) > 0;
  return (
    <>
      {changed && (
        <>
          <span className="add">+{git?.additions}</span>
          <span className="rem">−{git?.deletions}</span>
          <span>·</span>
        </>
      )}
      {git?.unpushed ? `${git.unpushed} unpushed` : "clean"}
      {pr ? ` · PR #${pr.number}` : ""}
    </>
  );
}

export function AgentScreen({ agentId }: { agentId: string }) {
  const agent = useStore((s) => agentOf(s, agentId));
  const project = useStore((s) => projectOf(s, agentId));
  const pop = useStore((s) => s.pop);
  const openSheet = useStore((s) => s.openSheet);
  const git = useStore((s) => s.gitStates[agentId]);
  const ensureAgent = useStore((s) => s.ensureAgent);
  const connected = useStore((s) => s.connection === "connected");
  const [tab, setTab] = useState<Tab>("chat");
  const [dir, setDir] = useState(1);
  // Owned here so sending a message can re-pin the log to the bottom.
  const pinnedToBottom = useRef(true);

  // An id with no record behind it: a notification tapped on a cold launch, or
  // a deep link, opens this screen before anything has listed the agent — and a
  // planning chat is never in the snapshot at all. Fetching it by id is what
  // turns the blank screen into the chat the tap named.
  const missing = agent === undefined;
  useEffect(() => {
    if (missing && connected) void ensureAgent(agentId);
  }, [missing, connected, agentId, ensureAgent]);

  if (!agent) return null;

  // A purpose-tagged workspace is a conversation, not a piece of work: the PM
  // never edits a file and the host denies it the publish ops, so Code and
  // Changes have nothing to show and the tab bar has nothing to choose between.
  const planning = !!agent.purpose;
  const pane = planning ? "chat" : tab;

  const go = (next: string) => {
    setDir(TABS.indexOf(next as Tab) > TABS.indexOf(tab) ? 1 : -1);
    setTab(next as Tab);
  };

  return (
    <>
      <Nav
        onBack={pop}
        backLabel={project?.name ?? "Back"}
        title={
          <>
            {agent.name}
            <ProviderMark id={agent.provider} />
          </>
        }
        sub={<Subtitle agentId={agentId} />}
        right={
          <button
            type="button"
            className="ibtn"
            onClick={() => openSheet("agentMore", { agentId })}
            aria-label="More"
          >
            <Icon name="more" size={20} />
          </button>
        }
      />
      {!planning && (
        <div className="ag-tabs">
          <Segmented
            items={[
              { id: "chat", label: "Chat" },
              { id: "code", label: "Code" },
              { id: "changes", label: "Changes", count: git?.files.length ?? 0 },
            ]}
            value={tab}
            onChange={go}
          />
        </div>
      )}
      <div className="ag-body">
        {agent.status === "spawning" ? (
          <SpawnPane task={agent.task} base={baseOf(agent)} branch={branchOf(agent)} />
        ) : (
          <div
            key={pane}
            className="pane"
            style={
              {
                "--dir": `${dir * 14}px`,
                display: "flex",
                flexDirection: "column",
                flex: 1,
                minHeight: 0,
              } as React.CSSProperties
            }
          >
            {pane === "chat" && <ChatTab agent={agent} pinRef={pinnedToBottom} />}
            {pane === "code" && <CodeTab agent={agent} />}
            {pane === "changes" && <ChangesTab agent={agent} onDelegated={() => go("chat")} />}
          </div>
        )}
      </div>
      {pane === "chat" && agent.status !== "spawning" && (
        <Composer
          agent={agent}
          onSend={() => {
            pinnedToBottom.current = true;
          }}
        />
      )}
    </>
  );
}
