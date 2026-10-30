import type { BoxImageStatus, BoxRuntimeProbe, BoxToolsStatus } from "@/api";

/** The providers boxes run (`boxes::supports_provider`), by label. Claude
 *  Code's disk is built by setup and gates readiness; the others' are built
 *  by setup when the host has the CLI, else on that provider's first launch. */
const PROVIDER_LABELS: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  cursor: "Cursor",
  opencode: "OpenCode",
};
const GATE = "claude";

/** Where the host's box setup stands, as one line: `busy` while the image
 *  downloads or a tools disk builds (`cancel` only while the download can
 *  still be stopped), `error` when either failed or the image is simply not
 *  there (Retry starts the install again, which also rebuilds the disk),
 *  `ready` once the image and Claude Code's disk are in. */
export type BoxSetup =
  | { state: "checking"; text: string }
  | { state: "busy"; text: string; cancel: boolean }
  | { state: "error"; text: string }
  | { state: "ready"; text: string };

export function boxSetup(
  probe: BoxRuntimeProbe | null,
  image: BoxImageStatus | null,
  tools: Record<string, BoxToolsStatus>,
): BoxSetup {
  if (!image) return { state: "checking", text: "Checking…" };
  if (probe && !probe.runtime_ok) return { state: "error", text: probe.problems.join("; ") };
  switch (image.phase) {
    case "downloading":
      return {
        state: "busy",
        text: `Downloading image${image.progress == null ? "" : ` ${image.progress}%`}`,
        cancel: true,
      };
    case "verifying":
      return { state: "busy", text: "Verifying image…", cancel: false };
    case "damaged":
    case "unavailable":
      return { state: "error", text: image.error ?? "The box image cannot be used." };
    case "not_installed":
      return { state: "error", text: "Image not installed." };
    default:
      break;
  }
  const label = (provider: string) => PROVIDER_LABELS[provider] ?? provider;
  const disks = Object.keys(PROVIDER_LABELS).map((provider) => ({
    provider,
    disk: tools[provider],
  }));
  const failed = disks.find(({ disk }) => disk?.phase === "failed");
  if (failed) {
    return {
      state: "error",
      text: `${label(failed.provider)} tools failed: ${failed.disk?.error ?? "unknown error"}`,
    };
  }
  const building = disks.find(
    ({ provider, disk }) =>
      disk?.phase === "building" || (provider === GATE && (!disk || disk.phase === "missing")),
  );
  if (building) {
    return { state: "busy", text: `Installing ${label(building.provider)} tools…`, cancel: false };
  }
  const ready = disks
    .filter(({ disk }) => disk?.phase === "ready")
    .map(({ provider, disk }) => `${label(provider)} ${disk?.version}`);
  return {
    state: "ready",
    text: `Ready · image v${image.version ?? image.pinned_version} · ${ready.join(" · ")} · other providers keep the previous sandbox`,
  };
}
