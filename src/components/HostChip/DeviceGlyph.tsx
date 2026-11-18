import { Icon } from "@/components/Icon";
import type { EnvironmentEntry } from "@/store/environments";
import { EnvironmentDot } from "./EnvironmentDot";

/** A machine, as a glyph: the laptop every physical device shares, This Mac
 *  and a paired host alike, so the glyph says what kind of place it is. A
 *  paired host carries its connection dot on the laptop's corner; This Mac
 *  has none, being always here. The dot is ringed in the surface under it
 *  (`--glyph-surface`), which whatever draws the glyph on another surface
 *  than a menu's sets. */
export function DeviceGlyph({ env, size = 14 }: { env: EnvironmentEntry; size?: number }) {
  return (
    <span className="device-glyph">
      <Icon name="laptop" size={size} />
      {env.kind === "remote" && <EnvironmentDot env={env} />}
    </span>
  );
}
