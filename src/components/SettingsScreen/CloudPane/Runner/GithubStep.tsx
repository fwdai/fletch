import { open as openExternal } from "@tauri-apps/plugin-shell";
import type { CloudRunnerProgress } from "@/api";
import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { DeviceCode } from "@/components/ui/DeviceCode";
import { Spinner } from "@/components/ui/Spinner";
import { plural } from "@/util/format";

/** The runner's own GitHub sign-in, the one step of a create that waits on
 *  the person: its code, large and copyable, and where to enter it. */
export function GithubStep({
  step,
}: {
  step: Extract<CloudRunnerProgress, { step: "github_login" }>;
}) {
  const minutes = Math.max(1, Math.round(step.expiresIn / 60));
  return (
    <div className="set-runner-github">
      <div className="set-gh-lede text-sm">
        The runner signs in to GitHub with a login of its own, to clone, push and open pull
        requests. Open GitHub and enter this code:
      </div>
      <DeviceCode code={step.userCode} verificationUri={step.verificationUrl} />
      <div>
        <Button
          variant="outline"
          size="sm"
          onClick={() => void openExternal(step.verificationUrl).catch(() => {})}
        >
          <Icon name="external" size={12} />
          Open GitHub
        </Button>
      </div>
      <div className="set-gh-hint text-sm flex-center set-runner-waiting">
        <Spinner size={12} />
        Waiting for the code to be entered. It lapses in {plural(minutes, "minute")}.
      </div>
    </div>
  );
}
