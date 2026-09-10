import { useEffect, useState } from "react";
import { Icon } from "../components/Icon";
import { Sheet } from "../components/ui";
import { baseOf, branchOf } from "../lib/agents";
import { agentOf, useStore } from "../store";

type Stage = "form" | "run" | "done" | "error";

/** The manual alternative to handing the git action to the agent (Changes tab):
 *  commit + push + open PR, or push to an existing one, with the user's own
 *  title and description. Each step is a real remote op; the progress list is
 *  just the three of them. */
export function PrSheet({
  open,
  onClose,
  agentId,
}: {
  open: boolean;
  onClose: () => void;
  agentId?: string;
}) {
  const agent = useStore((s) => (agentId ? agentOf(s.workspace, agentId) : undefined));
  const git = useStore((s) => (agentId ? s.gitStates[agentId] : undefined));
  const pr = useStore((s) => (agentId ? (s.prStates[agentId] ?? null) : null));
  const publish = useStore((s) => s.publish);
  const pushToPr = useStore((s) => s.pushToPr);
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [stage, setStage] = useState<Stage>("form");
  const [step, setStep] = useState(0);
  const [error, setError] = useState<string | null>(null);
  // Frozen when the run starts: creating a PR makes `pr` truthy, and the copy
  // must keep saying "opened", not flip to "updated" on success.
  const [pushedToExisting, setPushedToExisting] = useState(false);

  useEffect(() => {
    if (!open || !agent) return;
    // The form is the user's own text, never the agent's task: seeding it from
    // the task made the original prompt the commit message and PR body.
    setTitle(pr?.title ?? "");
    setBody("");
    setStage("form");
    setStep(0);
    setError(null);
  }, [open, agent, pr]);

  if (!agent || !agentId) return null;
  const existing = stage === "form" ? !!pr : pushedToExisting;
  const files = git?.files ?? [];
  const steps = [
    files.length
      ? `Commit ${files.length} file${files.length === 1 ? "" : "s"}`
      : "Nothing to commit",
    `Push ${branchOf(agent)}`,
    existing ? "Update pull request" : "Open pull request",
  ];

  const run = async () => {
    setPushedToExisting(!!pr);
    setStage("run");
    try {
      setStep(1);
      if (pr) {
        await pushToPr(agentId);
        setStep(3);
      } else {
        await publish(agentId, title.trim(), body);
        setStep(3);
      }
      setStage("done");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setStage("error");
    }
  };

  return (
    <Sheet
      open={open}
      onClose={stage === "run" ? undefined : onClose}
      full
      title={existing ? "Push to PR" : "Open pull request"}
      left={
        stage !== "run" ? (
          <button type="button" className="tbtn q" onClick={onClose}>
            {stage === "form" ? "Cancel" : "Close"}
          </button>
        ) : undefined
      }
      foot={
        stage === "form" ? (
          <button
            type="button"
            className="btn primary block"
            onClick={() => void run()}
            disabled={!title.trim()}
          >
            <Icon name="pr" size={17} />
            {existing ? `Commit & push to #${pr?.number}` : "Create pull request"}
          </button>
        ) : stage === "done" || stage === "error" ? (
          <button type="button" className="btn primary block" onClick={onClose}>
            Done
          </button>
        ) : null
      }
    >
      {stage === "form" && (
        <div className="pr-form pane">
          <div className="base" style={{ paddingTop: 6 }}>
            <Icon name="branch" size={12} />
            <b>{branchOf(agent)}</b>
            <Icon name="arrowR" size={12} />
            <b>{baseOf(agent)}</b>
            <span>·</span>
            <span>
              {files.length} file{files.length === 1 ? "" : "s"}
            </span>
          </div>
          <label htmlFor="pr-title">Title</label>
          <input id="pr-title" value={title} onChange={(e) => setTitle(e.target.value)} />
          <label htmlFor="pr-body">Description</label>
          <textarea id="pr-body" value={body} onChange={(e) => setBody(e.target.value)} />
        </div>
      )}
      {stage === "run" && (
        <div className="pane" style={{ paddingTop: 16 }}>
          <div className="progress">
            <i style={{ width: `${step * 33.4}%` }} />
          </div>
          <div className="steps">
            {steps.map((s, i) => (
              <div key={s} className={`step ${step > i + 1 ? "ok" : step === i + 1 ? "on" : ""}`}>
                <span className="ck">
                  <Icon name="check" size={11} sw={2.5} />
                </span>
                {s}
              </div>
            ))}
          </div>
        </div>
      )}
      {stage === "done" && (
        <div className="pr-done">
          <span className="big">
            <Icon name="pr" size={26} />
          </span>
          <h3>
            PR #{pr?.number} {existing ? "updated" : "opened"}
          </h3>
          <p>{pr?.title ?? title}</p>
          {pr && (
            <a href={pr.url} target="_blank" rel="noreferrer" style={{ marginTop: 10 }}>
              Open on GitHub
              <Icon name="external" size={13} />
            </a>
          )}
        </div>
      )}
      {stage === "error" && (
        <div className="empty">
          <b>That didn't work</b>
          {error}
        </div>
      )}
    </Sheet>
  );
}
