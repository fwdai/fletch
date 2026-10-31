import type { BoxImageStatus, BoxRuntimeProbe, BoxToolsStatus } from "@/api";
import { formatClockTime } from "@/util/format";

/** The providers boxes run (`boxes::supports_provider`), by label. Claude
 *  Code's disk is built by setup and gates readiness; the others' are built
 *  by setup when the host has the CLI, else on that provider's first launch. */
const PROVIDER_LABELS: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  cursor: "Cursor",
  opencode: "OpenCode",
  pi: "Pi",
};
const GATE = "claude";

/** Where the host's box setup stands, as one line: `busy` while the image
 *  downloads or a tools disk builds (`cancel` only while the download can
 *  still be stopped), `error` when either failed or the image is simply not
 *  there, `ready` once the image and Claude Code's disk are in. `retry` says
 *  what Retry starts again: the image install (which also builds the disk
 *  after it), or the failed provider's tools disk build alone, past the
 *  backoff after failed builds. */
export type BoxSetup =
  | { state: "checking"; text: string }
  | { state: "busy"; text: string; cancel: boolean }
  | { state: "error"; text: string; retry: "image" }
  | { state: "error"; text: string; retry: "tools"; provider: string }
  | { state: "ready"; text: string };

export function boxSetup(
  probe: BoxRuntimeProbe | null,
  image: BoxImageStatus | null,
  tools: Record<string, BoxToolsStatus>,
): BoxSetup {
  if (!image) return { state: "checking", text: "Checking…" };
  if (probe && !probe.runtime_ok) {
    return { state: "error", text: probe.problems.join("; "), retry: "image" };
  }
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
      return {
        state: "error",
        text: image.error ?? "The box image cannot be used.",
        retry: "image",
      };
    case "not_installed":
      return { state: "error", text: "Image not downloaded yet.", retry: "image" };
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
    // Nothing retries on its own at `retry_at`: the backoff only gates the
    // next launch or prebuild.
    const at = failed.disk?.retry_at;
    const next = at
      ? ` · Retry now, or it's tried again on a launch after ${formatClockTime(at)}`
      : "";
    return {
      state: "error",
      text: `${label(failed.provider)} tools failed: ${failed.disk?.error ?? "unknown error"}${next}`,
      retry: "tools",
      provider: failed.provider,
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
    text: `Ready · image v${image.version ?? image.pinned_version} · ${ready.join(" · ")} · other providers use the macOS sandbox`,
  };
}
