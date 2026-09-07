import { useState } from "react";
import { Icon } from "../../components/Icon";
import { Nav, ProviderMark, Segmented } from "../../components/ui";
import { baseOf, branchOf, isBusy, STATUS_LABEL } from "../../lib/agents";
import { fmtElapsed, useElapsed } from "../../lib/hooks";
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
  const agent = useStore((s) => agentOf(s.workspace, agentId));
  const startedAt = useStore((s) => s.turnStartedAt[agentId]);
  const pending = useStore((s) => Object.keys(s.pendingToolUse[agentId] ?? {}).length);
  const git = useStore((s) => s.gitStates[agentId]);
  const pr = useStore((s) => s.prStates[agentId]);
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
  const agent = useStore((s) => agentOf(s.workspace, agentId));
  const project = useStore((s) => projectOf(s.workspace, agentId));
  const pop = useStore((s) => s.pop);
  const openSheet = useStore((s) => s.openSheet);
  const git = useStore((s) => s.gitStates[agentId]);
  const [tab, setTab] = useState<Tab>("chat");
  const [dir, setDir] = useState(1);
  if (!agent) return null;

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
      <div className="ag-body">
        {agent.status === "spawning" ? (
          <SpawnPane task={agent.task} base={baseOf(agent)} branch={branchOf(agent)} />
        ) : (
          <div
            key={tab}
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
            {tab === "chat" && <ChatTab agent={agent} />}
            {tab === "code" && <CodeTab agent={agent} />}
            {tab === "changes" && <ChangesTab agent={agent} />}
          </div>
        )}
      </div>
      {tab === "chat" && agent.status !== "spawning" && <Composer agent={agent} />}
    </>
  );
}
