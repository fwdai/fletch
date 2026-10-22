// Row 03 · GitHub. A choice, not a check, and it looks like one: two buttons
// of equal weight, and nothing else to click until it is answered. Connecting
// runs the device flow inline and is what later lets Fletch clone, push and
// open pull requests; declining is a first-class answer, since the Git panel
// offers the same connection at the moment it is needed — the skipped row
// says so. The profile the sign-in returns also fills the account surfaces.

import { Icon } from "@/components/Icon";
import { Button } from "@/components/ui/Button";
import { DeviceCode } from "@/components/ui/DeviceCode";
import type { useGithubConnect } from "@/util/useGithubConnect";
import type { OnboardingSetup } from "../useSetup";
import { CheckRow } from "./CheckRow";
import type { CheckState } from "./usePreflight";

export function GithubRow({
  setup,
  github,
  state,
  active,
  onInspect,
  onSkip,
}: {
  setup: OnboardingSetup;
  github: ReturnType<typeof useGithubConnect>;
  state: CheckState;
  active: boolean;
  onInspect: () => void;
  onSkip: () => void;
}) {
  const { gh } = setup;
  const { connect, cancel, device, error, busy } = github;

  let detail: string;
  switch (state) {
    case "pending":
      detail = "Pull requests, straight from Fletch";
      break;
    case "checking":
      detail = device
        ? "Finish in your browser, then enter this code:"
        : busy
          ? "Starting GitHub sign-in…"
          : "Checking for a GitHub connection…";
      break;
    case "ok":
      detail = `Connected${gh?.login ? ` as ${gh.login}` : ""}`;
      break;
    case "skipped":
      detail = "Not now · the Git panel offers it when you open a pull request";
      break;
    default:
      detail = error ? "Couldn't connect" : "Optional · clone, push and open pull requests";
  }

  return (
    <CheckRow
      index={2}
      title="GitHub"
      detail={detail}
      state={state}
      active={active}
      onInspect={onInspect}
    >
      {busy && device && (
        <>
          <DeviceCode code={device.userCode} verificationUri={device.verificationUri} />
          <div className="pf-actions">
            <Button variant="link" size="sm" onClick={cancel}>
              Cancel
            </Button>
          </div>
        </>
      )}
      {state === "attention" && (
        <>
          {error && <p className="pf-err">{error}</p>}
          <div className="pf-actions">
            <Button variant="primary" onClick={() => void connect()}>
              <Icon name="github" size={13} />
              {error ? "Try again" : "Connect GitHub"}
            </Button>
            <Button variant="outline" onClick={onSkip}>
              Not now
            </Button>
          </div>
        </>
      )}
    </CheckRow>
  );
}
