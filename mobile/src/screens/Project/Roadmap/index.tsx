import { type ReactNode, useEffect, useMemo } from "react";
import { roadmapLists } from "../../../lib/roadmap";
import { useStore } from "../../../store";
import { CaptureField } from "./CaptureField";

/** The project's roadmap as far as the phone takes it: a capture field over two
 *  read-only lists — the Inbox, and what is up next. Ruling on any of it is the
 *  Mac's board, or a planning chat's. */
export function RoadmapPane({ projectId }: { projectId: string }) {
  const board = useStore((s) => s.boards[projectId]);
  const loadBoard = useStore((s) => s.loadBoard);
  const connected = useStore((s) => s.connection === "connected");
  const canCapture = useStore((s) => s.hostSupports("roadmap_capture"));
  const lists = useMemo(() => board && roadmapLists(board), [board]);

  // Read on open and again on every reconnect, like the planning chats: the
  // events that keep the lists live only reach a connected phone.
  useEffect(() => {
    if (connected) void loadBoard(projectId);
  }, [connected, projectId, loadBoard]);

  return (
    <div className="pane">
      {canCapture && <CaptureField projectId={projectId} />}
      {lists && (
        <>
          <Section title="Inbox">
            {lists.inbox.map(({ item, chip }) => (
              <Row key={item.id} title={item.title} sub={chip} code={item.code} />
            ))}
          </Section>
          <Section title="Next up">
            {lists.next.map((item) => (
              <Row
                key={item.id}
                title={item.title}
                sub={item.status === "queued" ? "Queued" : undefined}
                code={item.code}
              />
            ))}
          </Section>
          {lists.inbox.length === 0 && lists.next.length === 0 && (
            <div className="card" style={{ marginTop: 18 }}>
              <div className="empty">
                <b>Nothing here</b>
                Nothing captured, and nothing up next.
              </div>
            </div>
          )}
        </>
      )}
    </div>
  );
}

/** A titled card of rows, or nothing when there are none. */
function Section({ title, children }: { title: string; children: ReactNode[] }) {
  if (children.length === 0) return null;
  return (
    <>
      <div className="sect" style={{ marginTop: 18 }}>
        {title} <span className="n">{children.length}</span>
      </div>
      <div className="card">{children}</div>
    </>
  );
}

function Row({ title, sub, code }: { title: string; sub?: string; code: string }) {
  return (
    <div className="row">
      <div className="main">
        <div className="lbl">{title}</div>
        {sub && <div className="sub">{sub}</div>}
      </div>
      <span className="val mono">{code}</span>
    </div>
  );
}
