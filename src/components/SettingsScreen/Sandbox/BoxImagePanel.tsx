import { Button } from "@/components/ui/Button";
import { ProgressBar } from "@/components/ui/ProgressBar";
import { useAppStore } from "@/store";
import { SetRow } from "../primitives";

/** The pinned guest image's install state under the engine select, driven by
 *  `box_image_status` once and then every `box:image` (docs/boxes-spec.md §4):
 *  not installed → Install; downloading → a bar with the percent and Cancel;
 *  installed → the version; damaged or unavailable → the error and Install
 *  again. Mounted only while the box flag is on. */
export function BoxImagePanel({ disabled }: { disabled: boolean }) {
  const image = useAppStore((s) => s.boxImage);
  const install = useAppStore((s) => s.installBoxImage);
  const cancel = useAppStore((s) => s.cancelBoxImage);

  if (!image) {
    return <SetRow title="Box image" sub="Checking…" />;
  }

  const installing = image.phase === "downloading" || image.phase === "verifying";
  const broken = image.phase === "damaged" || image.phase === "unavailable";
  const sub = installing
    ? `${image.phase === "verifying" ? "Verifying" : "Downloading"} ${image.pinned_version}${
        image.progress == null ? "" : ` · ${image.progress}%`
      }`
    : image.phase === "installed"
      ? `Version ${image.version ?? image.pinned_version} installed.`
      : broken
        ? (image.error ?? "The installed image cannot be used.")
        : `Version ${image.pinned_version}, from ${image.url}.`;

  return (
    <>
      <SetRow title="Box image" sub={sub}>
        {installing ? (
          <Button variant="outline" onClick={() => void cancel()} disabled={disabled}>
            Cancel
          </Button>
        ) : image.phase !== "installed" ? (
          <Button variant="outline" onClick={() => void install()} disabled={disabled}>
            {broken ? "Install again" : "Install image"}
          </Button>
        ) : null}
      </SetRow>
      {installing && <ProgressBar percent={image.progress ?? null} className="set-box-progress" />}
    </>
  );
}
