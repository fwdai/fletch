import type { ForwardedPort } from "@/api";
import { Icon } from "@/components/Icon";
import { DropdownItem, MenuButton } from "@/components/ui";
import { PortLabel } from "./PortLabel";
import { portUrl } from "./PortLink";

/** The forwarded ports the run does not listen on, behind one trigger: the
 *  menu links each to its port on this Mac. */
export function OtherPorts({ ports }: { ports: ForwardedPort[] }) {
  const label = ports.length === 1 ? "1 other port" : `${ports.length} other ports`;
  return (
    <MenuButton
      anchorClassName="run-other-ports"
      trigger={(trigger) => (
        <button
          type="button"
          className="run-link text-xs tip"
          data-tip="Ports something else in the VM listens on"
          {...trigger}
        >
          {label}
          <Icon name="chevD" size={10} />
        </button>
      )}
    >
      {(close) =>
        ports.map((p) => (
          <DropdownItem
            key={p.guest_port}
            as="a"
            role="menuitem"
            className="run-port-item"
            href={portUrl(p)}
            target="_blank"
            rel="noreferrer"
            onClick={close}
          >
            <span className="di-l">
              <PortLabel port={p} />
            </span>
            <Icon name="external" size={10} />
          </DropdownItem>
        ))
      }
    </MenuButton>
  );
}
