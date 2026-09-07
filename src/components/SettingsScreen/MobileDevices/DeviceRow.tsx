import type { RemoteDevice } from "@/api";
import { Button } from "@/components/ui/Button";
import { formatAge } from "@/util/format";

const PLATFORM_LABELS: Record<string, string> = {
  ios: "iOS",
  android: "Android",
  macos: "macOS",
};

/** One paired device: name, platform, whether it is connected right now, and
 *  the credential's revoke. Revoking drops the stored token hash — the phone's
 *  next `hello` is refused, and it has to be paired again. */
export function DeviceRow({
  device,
  disabled,
  onRevoke,
}: {
  device: RemoteDevice;
  disabled?: boolean;
  onRevoke: () => void;
}) {
  const platform = PLATFORM_LABELS[device.platform] ?? device.platform;
  const seen = device.lastSeenAt ? formatAge(device.lastSeenAt, Date.now()) : null;
  const sub = device.connected
    ? `${platform} · connected`
    : seen
      ? `${platform} · last seen ${seen} ago`
      : `${platform} · never connected`;

  return (
    <div className="set-row flex-center">
      <div className="set-row-l">
        <div className="set-row-t text-base flex-center">
          <i className="set-dev-dot" data-on={device.connected ? "1" : "0"} />
          {device.name}
        </div>
        <div className="set-row-s text-sm">{sub}</div>
      </div>
      <div className="set-row-c flex-center">
        <Button variant="outline" size="sm" danger disabled={disabled} onClick={onRevoke}>
          Revoke
        </Button>
      </div>
    </div>
  );
}
