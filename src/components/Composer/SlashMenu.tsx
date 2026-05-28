import { useEffect, useRef } from "react";
import type { SlashCommand } from "../../data/slashCommands";

interface Props {
  commands: SlashCommand[];
  highlight: number;
  onPick: (cmd: SlashCommand) => void;
  onHighlight: (i: number) => void;
}

export function SlashMenu({ commands, highlight, onPick, onHighlight }: Props) {
  const activeRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest" });
  }, [highlight]);

  if (commands.length === 0) return null;

  return (
    <div
      className="dd slash-menu"
      style={{ bottom: "calc(100% + 6px)", left: 0, right: 0 }}
      role="listbox"
    >
      <div className="dd-sect">Slash commands</div>
      {commands.map((cmd, i) => (
        <div
          key={cmd.name}
          ref={i === highlight ? activeRef : undefined}
          className={`dd-item slash-item ${i === highlight ? "active" : ""}`}
          role="option"
          aria-selected={i === highlight}
          onMouseDown={(e) => {
            e.preventDefault();
            onPick(cmd);
          }}
          onMouseEnter={() => onHighlight(i)}
        >
          <span className="slash-name">
            /{cmd.name}
            {cmd.hint && <span className="slash-hint"> {cmd.hint}</span>}
          </span>
          <span className="slash-desc">{cmd.description}</span>
        </div>
      ))}
    </div>
  );
}
