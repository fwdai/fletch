import { SetToggle } from "@/components/SettingsScreen/primitives";

/** A toggle-list for assigning shared library items (skills, MCP servers) to a
 *  custom agent. Selection is an ordered id array: toggling on appends, so the
 *  spawn snapshot preserves the order items were assigned in. A `disabled`
 *  item can't be toggled ON (the base can't deliver it — see `MCP_SUPPORT`),
 *  but an already-selected one can still be toggled OFF so stale assignments
 *  are cleanable after a base switch. */
export function AssignPicker({
  items,
  selected,
  onChange,
  emptyHint,
}: {
  items: { id: string; name: string; detail?: string; disabled?: boolean }[];
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
      {items.map((item) => {
        const on = selected.includes(item.id);
        return (
          <div
            key={item.id}
            className="set-row flex-center"
            style={item.disabled ? { opacity: 0.55 } : undefined}
          >
            <div className="set-row-l">
              <div className="set-row-t text-base">{item.name}</div>
              {item.detail && <div className="set-row-s text-sm truncate">{item.detail}</div>}
            </div>
            <div className="set-row-c flex-center">
              <SetToggle on={on} disabled={item.disabled && !on} onClick={() => toggle(item.id)} />
            </div>
          </div>
        );
      })}
    </div>
  );
}
