// ContainerErrors.tsx — the shared inline error strip for a container node.

import { Icon } from "../../../components/Icon";

export function ContainerErrors({ errors }: { errors: string[] | undefined }) {
  if (!errors) return null;
  return (
    <div className="wb-errs">
      {errors.map((msg) => (
        <span className="wb-err" key={msg}>
          <Icon name="close" size={9} /> {msg}
        </span>
      ))}
    </div>
  );
}
