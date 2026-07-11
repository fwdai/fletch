import { SetToggle } from "@/components/SettingsScreen/primitives";

/** A toggle-list for assigning shared library items (skills, MCP servers) to a
 *  custom agent. Selection is an ordered id array: toggling on appends, so the
 *  spawn snapshot preserves the order items were assigned in. */
export function AssignPicker({
  items,
  selected,
  onChange,
  emptyHint,
}: {
  items: { id: string; name: string; detail?: string }[];
  selected: string[];
  onChange: (next: string[]) => void;
  emptyHint: string;
}) {
  if (items.length === 0) {
    return <div className="ca-field-hint text-sm">{emptyHint}</div>;
  }

  const toggle = (id: string) =>
    onChange(selected.includes(id) ? selected.filter((x) => x !== id) : [...selected, id]);

  return (
    <div className="set-rows">
      {items.map((item) => (
        <div key={item.id} className="set-row flex-center">
          <div className="set-row-l">
            <div className="set-row-t text-base">{item.name}</div>
            {item.detail && <div className="set-row-s text-sm truncate">{item.detail}</div>}
          </div>
          <div className="set-row-c flex-center">
            <SetToggle on={selected.includes(item.id)} onClick={() => toggle(item.id)} />
          </div>
        </div>
      ))}
    </div>
  );
}
