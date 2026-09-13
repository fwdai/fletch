import { Icon } from "@desktop/components/Icon";
import { useStore } from "../../store";

/** Home with a live host and nothing on it. The two ways a project gets here
 *  from the phone are the actions, each opening Add project on its own tab;
 *  the third way — pinning a repo in Fletch on the Mac — is mentioned so a
 *  desktop user is not left wondering why this is empty. */
export function EmptyProjects() {
  const openSheet = useStore((s) => s.openSheet);
  const host = useStore((s) => s.hostInfo?.name) ?? "your Mac";
  return (
    <div className="blank fade">
      <div className="glyph accent">
        <Icon name="folder" size={30} strokeWidth={1.5} />
      </div>
      <h2>No projects yet</h2>
      <div className="body">
        <p>A project is a Git repo on {host}. Add one, then start an agent in it.</p>
      </div>
      <div className="card ways">
        <button type="button" className="row" onClick={() => openSheet("addProject")}>
          <span className="ic">
            <Icon name="folder" size={17} />
          </span>
          <div className="main">
            <div className="lbl">Open a folder</div>
            <div className="sub">A repo already on your Mac</div>
          </div>
          <Icon name="chevR" size={16} className="chev" />
        </button>
        <button
          type="button"
          className="row"
          onClick={() => openSheet("addProject", { tab: "clone" })}
        >
          <span className="ic">
            <Icon name="github" size={17} />
          </span>
          <div className="main">
            <div className="lbl">Clone from GitHub</div>
            <div className="sub">Into a folder you choose</div>
          </div>
          <Icon name="chevR" size={16} className="chev" />
        </button>
      </div>
      <div className="hint">Repos you pin in Fletch on your Mac show up here too.</div>
    </div>
  );
}
