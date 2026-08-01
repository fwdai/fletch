import { Icon } from "@/components/Icon";
import { Loader } from "@/components/ui/Loader";
import { QuestionCard } from "@/components/Workspace/messages/UserInput/QuestionCard";
import type { PmMessage } from "../types";
import type { RoadmapState } from "../useRoadmap";
import { Probe } from "./Probe";
import { Proposal } from "./Proposal";

/** One thread message. The user bubble and agent prose reuse the transcript's
 *  `.m-user` / `.m-agent` skins, and a clarifying question reuses the agent
 *  view's question card verbatim — the PM asks in the same vocabulary. */
export function Message({ msg, roadmap }: { msg: PmMessage; roadmap: RoadmapState }) {
  switch (msg.kind) {
    case "user":
      return <div className="m-user">{msg.body}</div>;

    case "text":
      return <div className="m-agent">{msg.body}</div>;

    case "thinking":
      return <Thinking body={msg.body} />;

    case "probe":
      return <Probe summary={msg.summary} findings={msg.findings} />;

    case "question":
      return (
        <QuestionCard
          question={msg.question}
          index={0}
          total={1}
          committed={!!msg.answer}
          answer={msg.answer ?? null}
          onAnswer={(a) => roadmap.answerQuestion(msg.id, a)}
        />
      );

    case "proposal":
      return (
        <Proposal
          note={msg.note}
          changes={msg.changes}
          resolved={msg.resolved}
          onAccept={() => roadmap.accept(msg.id)}
          onDiscard={() => roadmap.discard(msg.id)}
        />
      );

    case "landed":
      return (
        <div className="rm-landed flex-center mono text-xs">
          <Icon name="map" size={11} />
          {msg.codes.map((c) => (
            <button
              key={c}
              type="button"
              className="rm-landed-code"
              onClick={() => roadmap.focusItem(c)}
            >
              {c}
            </button>
          ))}
          <span>on the board</span>
        </div>
      );
  }
}

/** The PM narrating what it's about to check. Also the live placeholder while
 *  a beat is still arriving (`body` is the generic line then). Same skin as the
 *  transcript's "agent is working" line. */
export function Thinking({ body }: { body: string }) {
  return (
    <div className="writing flex-center">
      <Loader />
      <span>{body}</span>
    </div>
  );
}
