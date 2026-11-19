import type { CloudProviderStatus, CloudRunnerProgress } from "@/api";
import { Icon } from "@/components/Icon";
import { Spinner } from "@/components/ui/Spinner";
import { StatusLine } from "../StatusLine";
import { GithubStep } from "./GithubStep";
import { stepWords } from "./words";

/** A create under way: each step the engine has reported, the last one the
 *  one it is on, and at the GitHub step what the person has to do. */
export function Creating({
  provider,
  steps,
}: {
  provider: CloudProviderStatus;
  steps: CloudRunnerProgress[];
}) {
  const current = steps.at(-1);
  return (
    <>
      <StatusLine status="Creating the runner…" />
      {steps.length > 0 && (
        <ol className="set-runner-steps text-sm">
          {steps.map((step, i) => {
            const done = i < steps.length - 1;
            return (
              // A create reports each kind of step once.
              <li key={step.step} className={done ? "done" : "current"}>
                {done ? <Icon name="check" size={12} /> : <Spinner size={12} />}
                {stepWords(step, provider.label)}
              </li>
            );
          })}
        </ol>
      )}
      {current?.step === "github_login" && <GithubStep step={current} />}
      <div className="set-gh-hint text-sm">
        This takes a few minutes. Keep Fletch open until it is done.
      </div>
    </>
  );
}
