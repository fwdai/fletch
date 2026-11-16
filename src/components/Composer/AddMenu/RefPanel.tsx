import type { IconName } from "@/components/Icon";
import { FlyoutGlyph, SideFlyout } from "@/components/ui/FlyoutMenu";
import { type RefEntry, RefList } from "./RefList";

/** One of the add menu's side panels: a heading over a filterable list of
 *  things to put into the prompt. `kind` keys the panel, so moving from one
 *  row to the next swaps the list (and its filter) in place. */
export function RefPanel({
  kind,
  icon,
  title,
  tag,
  entries,
  placeholder,
  empty,
  onPick,
}: {
  kind: string;
  icon: IconName;
  title: string;
  /** What one entry is, in a word, at the heading's far end. */
  tag: string;
  /** Null while the list is still loading. */
  entries: RefEntry[] | null;
  placeholder: string;
  empty: string;
  onPick: (id: string) => void;
}) {
  return (
    <SideFlyout flyKey={kind} icon={<FlyoutGlyph name={icon} head />} title={title} tag={tag} fill>
      <RefList entries={entries} placeholder={placeholder} empty={empty} onPick={onPick} />
    </SideFlyout>
  );
}
