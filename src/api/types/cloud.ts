// Cloud providers: the services whose machines a workspace can run on instead
// of this Mac. The frontend mirror of what `cloud_providers` answers, and of
// the `cloud_runner_*` commands (`crates/fletch-core/src/cloud_runner`). All a
// surface knows of a provider is its status here, so none is named in `src/`.

/** A provider's id, as the backend spells it. Any string: a surface renders
 *  the providers `cloud_providers` lists and never branches on one. */
export type CloudProviderId = string;

/** One provider on this Mac, connected or not, and the words it is shown,
 *  connected and explained in. */
export interface CloudProviderStatus {
  id: CloudProviderId;
  /** How the provider is named on screen. */
  label: string;
  /** A token for it is kept in this Mac's keychain. */
  connected: boolean;
  /** Who the token acts for, of the kind `account_label` names. `null` while
   *  not connected. */
  account: string | null;
  /** What the provider calls an account, as a field label: "Organization". */
  account_label: string;
  /** The account an empty account field connects, as its placeholder. */
  account_placeholder: string;
  /** What a token looks like, as the token field's placeholder. */
  token_placeholder: string;
  /** One sentence on where to get a token. */
  token_help: string;
  /** One short sentence on what the provider bills for a workspace there. */
  billing: string;
}

/** Where the runner is (`cloud_runner_status`): none, being set up, a set-up
 *  that did not finish, or its machine's state as its provider reports it. */
export type CloudRunnerState =
  | { state: "none" }
  /** A create is under way in this app now. */
  | { state: "setting_up" }
  /** A create that did not finish (the app quit during it); its machine may
   *  be at the provider. Delete clears it. */
  | { state: "incomplete" }
  | { state: "starting" }
  | { state: "running" }
  | { state: "stopping" }
  | { state: "stopped" }
  /** The machine is gone and the disk is not: a start makes a new machine on
   *  the disk, and it is the same runner. */
  | { state: "machine_lost" }
  /** The disk is gone, and its history, logins and pairings with it. Never
   *  made again on an empty disk. */
  | { state: "disk_lost" }
  | { state: "failed"; reason: string };

/** A runner that is set up: what a device needs to reach it through the
 *  relay. */
export interface CloudRunnerRecord {
  /** Its host public key (base64url): its identity, and its route on the
   *  relay. */
  hostId: string;
  /** What paired devices call it. */
  name: string;
  /** The relay it dials. */
  relay: string;
}

/** What `cloud_runner_status` answers: the state, the provider its machine is
 *  at (`null` while there is none), and its record once it is set up. */
export type CloudRunnerStatus = CloudRunnerState & {
  provider: CloudProviderId | null;
  record: CloudRunnerRecord | null;
};

/** What `cloud_runner_create` is given beside the provider. */
export interface CloudRunnerInputs {
  /** This device's public key as a client (`remote_device_public_key`),
   *  authorised on the runner with full access. */
  devicePublicKey: string;
  /** What the runner's device list calls this device. */
  deviceName: string | null;
  /** What this device runs on (`macos`). */
  devicePlatform: string | null;
  /** A long-lived Claude Code token (`claude setup-token`), kept on the
   *  runner's disk alone. */
  claudeToken: string;
  /** What paired devices call the runner; the engine's default when null or
   *  blank. */
  name: string | null;
}

/** A step of a runner create, as `cloud-runner:progress` reports it when it
 *  starts. In order; the GitHub step carries what the person types and
 *  where. */
export type CloudRunnerProgress =
  | { step: "creating_machine" }
  | { step: "connecting" }
  /** `version` is null for a development build pushed from this Mac. */
  | { step: "installing_host"; version: string | null }
  /** `cli` is the agent CLI's label. */
  | { step: "installing_cli"; cli: string; version: string }
  | { step: "configuring" }
  | { step: "starting_service" }
  | { step: "connecting_provider" }
  | { step: "authorizing_device" }
  /** The runner's own GitHub sign-in: `userCode` entered at
   *  `verificationUrl` within `expiresIn` seconds. */
  | { step: "github_login"; userCode: string; verificationUrl: string; expiresIn: number };

/** What `cloud_runner_delete` did. */
export interface CloudRunnerDeleted {
  /** The workspaces' machines the runner destroyed, by agent id. */
  destroyed: string[];
  /** Set when a forced delete may have left workspaces' machines at the
   *  provider: what, for the person to delete there. */
  warning: string | null;
}

/** A pairing code for another device, minted by the running runner. */
export interface CloudRunnerPairing {
  /** The `fletch://pair` link, for a QR code. */
  url: string;
  /** The code the link carries. */
  code: string;
  /** When the code lapses (RFC 3339). */
  expiresAt: string;
  /** The access it grants. */
  preset: string;
}
