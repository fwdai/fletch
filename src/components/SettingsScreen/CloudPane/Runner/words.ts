import type { CloudRunnerProgress, CloudRunnerStatus } from "@/api";

/** A step of a runner create in plain words. `label` is the provider's name. */
export function stepWords(step: CloudRunnerProgress, label: string): string {
  switch (step.step) {
    case "creating_machine":
      return `Creating its machine and disk at ${label}`;
    case "connecting":
      return "Connecting to the machine";
    case "installing_host":
      return step.version
        ? `Installing Fletch ${step.version}`
        : "Installing this Mac's build of Fletch";
    case "installing_cli":
      return `Installing ${step.cli} ${step.version}`;
    case "configuring":
      return "Saving the Claude token on its disk";
    case "starting_service":
      return "Restarting the machine and starting Fletch on it";
    case "connecting_provider":
      return `Giving it this Mac's ${label} connection`;
    case "authorizing_device":
      return "Letting this Mac connect to it";
    case "github_login":
      return "Signing it in to GitHub";
  }
}

/** Where a runner is: a few words for its status line, and what that means
 *  for the person, when there is more to say. */
export function stateWords(
  status: CloudRunnerStatus,
  label: string,
): { short: string; detail: string | null } {
  switch (status.state) {
    case "none":
      return { short: "no runner", detail: null };
    case "setting_up":
      return { short: "being set up…", detail: "This page updates when it is done." };
    case "incomplete":
      return {
        short: "set-up did not finish",
        detail: `Fletch quit while creating it, so its machine may still be at ${label}. Delete it, then create a new one.`,
      };
    case "starting":
      return { short: "starting…", detail: null };
    case "running":
      return {
        short: "running",
        detail:
          "Agents on it keep running with this Mac closed. This Mac and the devices you pair reach it through Fletch's relay.",
      };
    case "stopping":
      return { short: "stopping…", detail: null };
    case "stopped":
      return {
        short: "stopped",
        detail: `Its agents don't run and no device can reach it until you start it. ${label} still bills for its disk.`,
      };
    case "machine_lost":
      return {
        short: "machine lost",
        detail: `Its machine is gone at ${label}; its disk is not. Start makes a new machine on the disk, and it is the same runner.`,
      };
    case "disk_lost":
      return {
        short: "disk lost",
        detail: `Its disk is gone at ${label}, and its history, logins and pairings with it. It can't be recovered: delete it and create a new one.`,
      };
    case "failed":
      return { short: "failed", detail: `${label} can't run it: ${status.reason}` };
  }
}

/** What can be done to a runner in `state`. Delete is always offered except
 *  while a create is setting it up. */
export function runnerActions(state: CloudRunnerStatus["state"]) {
  return {
    start: state === "stopped" || state === "machine_lost" || state === "failed",
    stop: state === "running" || state === "starting",
    pair: state === "running",
    delete: state !== "setting_up" && state !== "none",
  };
}
