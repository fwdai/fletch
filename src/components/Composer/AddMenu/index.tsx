import { useMemo, useState } from "react";
import type { PrSummary, RoadmapItem, TrackerIssue } from "@/api";
import { Icon } from "@/components/Icon";
import {
  FlyoutFoot,
  FlyoutGlyph,
  FlyoutMenu,
  FlyoutRow,
  FlyoutSection,
} from "@/components/ui/FlyoutMenu";
import { IconButton } from "@/components/ui/IconButton";
import { useCachedList } from "../useCachedList";
import { issueEntries, issueId, prEntries, roadmapEntries } from "./entries";
import type { RefEntry } from "./RefList";
import { RefPanel } from "./RefPanel";

/** The row whose side panel is showing. */
type Fly = "issues" | "roadmap" | "prs" | "commands";

/** Something `/` invokes, under the token that invokes it. */
export interface CommandChoice {
  command: string;
  description: string;
  /** The heading it is listed under, where the list has several kinds. */
  group?: string;
}

interface Props {
  /** Open the file picker and attach what it returns. */
  onAttachFiles: () => void;
  /** Open the folder picker. Omit where a folder can't be given to the agent. */
  onAttachFolders?: () => void;
  /** Open tracker issues (GitHub, Linear) to work on. Omit to hide the row. */
  issues?: { list: () => Promise<TrackerIssue[]>; onPick: (issue: TrackerIssue) => void };
  /** The project's roadmap, of which the items an agent can pick up are
   *  offered. Omit to hide the row. */
  roadmap?: { list: () => Promise<RoadmapItem[]>; onPick: (item: RoadmapItem) => void };
  /** The repo's open pull requests to reference. Omit to hide the row. */
  prs?: { list: () => Promise<PrSummary[]>; onPick: (pr: PrSummary) => void };
  /** What `/` completes here: slash commands, and skills where they can be
   *  invoked. Omit to hide the row. */
  commands?: { choices: CommandChoice[]; onPick: (command: string) => void };
  /** The workspace's VM network for this turn: whether it is open, and the
   *  switch. Omit where there is nothing to open. */
  network?: { open: boolean; onToggle: (open: boolean) => void };
  /** Typing `@` completes a file here, so the card's hint may say so. */
  fileMentions?: boolean;
  /** The menu closed, with or without a pick: the composer takes the focus
   *  back from the filter. */
  onClose?: () => void;
}

/** The composer's one "add to the prompt" control: a `+` that opens every way
 *  of putting something into this message besides typing it. Files attach and
 *  folders are referenced; an issue, a roadmap item, a pull request or a
 *  command is picked from a list beside the card (what `#` and `/` complete
 *  inline, for a user who hasn't met them); and the turn's network opens from
 *  here.
 *
 *  It sits at the right of the footer, so the card lines its right edge up
 *  with the button and its lists open over whatever is to the right of the
 *  conversation. */
export function AddMenu({
  onAttachFiles,
  onAttachFolders,
  issues,
  roadmap,
  prs,
  commands,
  network,
  fileMentions,
  onClose,
}: Props) {
  const [open, setOpen] = useState(false);
  const [fly, setFly] = useState<Fly | null>(null);

  // Fetched as its panel first shows, then kept briefly: the lists are remote
  // (`gh`, Linear) or the backend's, and a pointer crossing the rows must not
  // refetch them.
  const issueList = useCachedList(issues?.list, fly === "issues");
  const roadmapList = useCachedList(roadmap?.list, fly === "roadmap");
  const prList = useCachedList(prs?.list, fly === "prs");

  const issueRows = useMemo(() => issueList && issueEntries(issueList), [issueList]);
  const roadmapRows = useMemo(() => roadmapList && roadmapEntries(roadmapList), [roadmapList]);
  const prRows = useMemo(() => prList && prEntries(prList), [prList]);
  const commandRows = useMemo<RefEntry[]>(
    () =>
      (commands?.choices ?? []).map((c) => ({
        id: c.command,
        tag: `/${c.command}`,
        title: c.description,
        group: c.group,
      })),
    [commands?.choices],
  );
  // Named for what its list holds: skills join the commands only where they
  // can be invoked.
  const hasSkills = commands?.choices.some((c) => c.group === "Skills") ?? false;

  function close() {
    setOpen(false);
    setFly(null);
    onClose?.();
  }

  /** A row or a list entry was chosen: the menu goes, then the choice acts. */
  function choose(act: () => void) {
    close();
    act();
  }

  const flyout =
    fly === "issues" && issues ? (
      <RefPanel
        kind="issues"
        icon="issue"
        title="Open issues"
        tag="issue"
        entries={issueRows}
        placeholder="Filter issues"
        empty="No open issues"
        onPick={(id) => {
          const issue = issueList?.find((i) => issueId(i) === id);
          if (issue) choose(() => issues.onPick(issue));
        }}
      />
    ) : fly === "roadmap" && roadmap ? (
      <RefPanel
        kind="roadmap"
        icon="map"
        title="Roadmap"
        tag="item"
        entries={roadmapRows}
        placeholder="Filter roadmap items"
        empty="Nothing on the roadmap to pick up"
        onPick={(id) => {
          const item = roadmapList?.find((i) => i.id === id);
          if (item) choose(() => roadmap.onPick(item));
        }}
      />
    ) : fly === "prs" && prs ? (
      <RefPanel
        kind="prs"
        icon="pr"
        title="Pull requests"
        tag="pr"
        entries={prRows}
        placeholder="Filter pull requests"
        empty="No open pull requests"
        onPick={(id) => {
          const pr = prList?.find((p) => String(p.number) === id);
          if (pr) choose(() => prs.onPick(pr));
        }}
      />
    ) : fly === "commands" && commands ? (
      <RefPanel
        kind="commands"
        icon="terminal"
        title={hasSkills ? "Commands and skills" : "Commands"}
        tag="/"
        entries={commandRows}
        placeholder={hasSkills ? "Filter commands and skills" : "Filter commands"}
        empty="No commands for this agent"
        onPick={(command) => choose(() => commands.onPick(command))}
      />
    ) : null;

  /** A row that opens a side panel: lit while its panel shows. */
  const opener = (kind: Fly) => ({
    "aria-haspopup": "menu" as const,
    "aria-expanded": fly === kind,
    hot: fly === kind,
    chevron: true,
    onMouseEnter: () => setFly(kind),
    onClick: () => setFly(kind),
  });

  return (
    <span className="add-menu">
      <IconButton
        className="composer-action"
        tip={open ? undefined : "Add to the prompt"}
        aria-label="Add to the prompt"
        aria-haspopup="menu"
        aria-expanded={open}
        active={open}
        onClick={() => (open ? close() : setOpen(true))}
      >
        <Icon name="plus" size={15} />
      </IconButton>

      {open && (
        <FlyoutMenu
          align="end"
          sideWidth={300}
          // One height for the card and its lists, whichever rows this
          // composer has: the card is a handful of rows, a list is not.
          minHeight={300}
          onClose={close}
          flyout={flyout}
        >
          <FlyoutSection>Add to the prompt</FlyoutSection>
          <FlyoutRow
            icon={<FlyoutGlyph name="attach" />}
            label="Attach files"
            onMouseEnter={() => setFly(null)}
            onClick={() => choose(onAttachFiles)}
          />
          {onAttachFolders && (
            <FlyoutRow
              icon={<FlyoutGlyph name="folder" />}
              // "Reference", not "attach": nothing is copied or sent.
              label="Reference a folder"
              title="The agent reads the folder where it is, read-only until you say otherwise. Nothing is copied."
              onMouseEnter={() => setFly(null)}
              onClick={() => choose(onAttachFolders)}
            />
          )}
          {issues && (
            <FlyoutRow
              icon={<FlyoutGlyph name="issue" />}
              label="Work on an issue"
              {...opener("issues")}
            />
          )}
          {roadmap && (
            <FlyoutRow
              icon={<FlyoutGlyph name="map" />}
              label="Work on a roadmap item"
              {...opener("roadmap")}
            />
          )}
          {prs && (
            <FlyoutRow
              icon={<FlyoutGlyph name="pr" />}
              label="Reference a pull request"
              {...opener("prs")}
            />
          )}
          {commands && (
            <FlyoutRow
              icon={<FlyoutGlyph name="terminal" />}
              label={hasSkills ? "Run a command or skill" : "Run a command"}
              {...opener("commands")}
            />
          )}
          {network && (
            <>
              <FlyoutSection>This turn</FlyoutSection>
              <FlyoutRow
                icon={<FlyoutGlyph name="globe" />}
                label={network.open ? "Close the network" : "Open the network"}
                note={network.open ? "open" : undefined}
                title={
                  network.open
                    ? "The network is open until this turn ends"
                    : "Let the agent reach any host until this turn ends"
                }
                active={network.open}
                onMouseEnter={() => setFly(null)}
                onClick={() => choose(() => network.onToggle(!network.open))}
              />
            </>
          )}
          <FlyoutFoot>
            <span>Or type</span>
            {fileMentions && (
              <span>
                <kbd className="add-key">@</kbd> file
              </span>
            )}
            {prs && (
              <span>
                <kbd className="add-key">#</kbd> pull request
              </span>
            )}
            <span>
              <kbd className="add-key">/</kbd> command
            </span>
          </FlyoutFoot>
        </FlyoutMenu>
      )}
    </span>
  );
}
