import { Icon } from "@/components/Icon";
import { DropdownItem, DropdownSection } from "@/components/ui/Dropdown";
import type { ForkChoice } from "./options";

/** A titled set of mutually exclusive menu rows, the selected one checked.
 *  Picking a row only selects it; the menu stays open. */
export function ChoiceGroup<T extends string>({
  title,
  choices,
  value,
  onChange,
}: {
  title: string;
  choices: ForkChoice<T>[];
  value: T;
  onChange: (value: T) => void;
}) {
  return (
    <div role="group" aria-label={title}>
      <DropdownSection>{title}</DropdownSection>
      {choices.map((choice) => {
        const selected = choice.value === value;
        return (
          <DropdownItem
            key={choice.value}
            as="button"
            role="menuitemradio"
            aria-checked={selected}
            active={selected}
            onClick={() => onChange(choice.value)}
          >
            <span className="di-i">{selected && <Icon name="check" size={12} />}</span>
            <span className="di-l">{choice.label}</span>
          </DropdownItem>
        );
      })}
    </div>
  );
}
