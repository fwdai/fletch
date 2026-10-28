import type { BoxImageStatus, BoxRuntimeProbe, BoxToolsStatus } from "@/api";

/** The provider whose tools disk the setup line reports. Claude Code is the
 *  one provider boxes run today (docs/boxes-spec.md §13). */
const PROVIDER = "claude";
const PROVIDER_LABEL = "Claude Code";

/** Where the host's box setup stands, as one line: `busy` while the image
 *  downloads or the tools disk builds (`cancel` only while the download can
 *  still be stopped), `error` when either failed or the image is simply not
 *  there (Retry starts the install again, which also rebuilds the disk),
 *  `ready` once both are in. */
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
  const disk = tools[PROVIDER];
  if (!disk || disk.phase === "building" || disk.phase === "missing") {
    return { state: "busy", text: `Installing ${PROVIDER_LABEL} tools…`, cancel: false };
  }
  if (disk.phase === "failed") {
    return {
      state: "error",
      text: `${PROVIDER_LABEL} tools failed: ${disk.error ?? "unknown error"}`,
    };
  }
  return {
    state: "ready",
    text: `Ready · image v${image.version ?? image.pinned_version} · ${PROVIDER_LABEL} ${disk.version} · other providers keep the previous sandbox`,
  };
}
