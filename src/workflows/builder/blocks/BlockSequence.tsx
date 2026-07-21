// BlockSequence.tsx — renders a sequence of blocks as a vertical pipeline with
// connectors (each offering an insert point) and a trailing "Add block" button.
// Used at the top level and (recursively) for loop bodies, so it takes its
// owning sequence's nid (`null` = top level).

import { Fragment, useState } from "react";
import { Icon } from "../../../components/Icon";
import { BLOCK_TYPES, type BlockTypeDef } from "../../data";
import type { BuilderCtx } from "../ctx";
import { useDismissOnViewportChange } from "../hooks";
import type { EBlock, NodeId } from "../model";
import { LoopContainer } from "./LoopContainer";
import { OrchestrateContainer } from "./OrchestrateContainer";
import { ParallelContainer } from "./ParallelContainer";
import { StepCard } from "./StepCard";

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

function BlockNode({
  block,
  index,
  ctx,
  canRemove,
}: {
  block: EBlock;
  index: number;
  ctx: BuilderCtx;
  canRemove: boolean;
}) {
  if (block.kind === "step") {
    return <StepCard step={block} ctx={ctx} indexLabel={pad2(index + 1)} canRemove={canRemove} />;
  }
  if (block.kind === "parallel") {
    return <ParallelContainer block={block} ctx={ctx} indexLabel={pad2(index + 1)} />;
  }
  if (block.kind === "loop") {
    return <LoopContainer block={block} ctx={ctx} indexLabel={pad2(index + 1)} />;
  }
  return <OrchestrateContainer block={block} ctx={ctx} indexLabel={pad2(index + 1)} />;
}

/** The fixed-positioned block-type menu shared by connectors and "Add block". */
function BlockMenu({
  at,
  types,
  onPick,
  onClose,
}: {
  at: { top: number; left: number };
  types: BlockTypeDef[];
  onPick: (type: BlockTypeDef["id"]) => void;
  onClose: () => void;
}) {
  return (
    <>
      <div style={{ position: "fixed", inset: 0, zIndex: 55 }} onClick={onClose} />
      <div className="dd wb-add-menu" style={{ position: "fixed", top: at.top, left: at.left }}>
        {types.map((t) => (
          <div
            key={t.id}
            className="dd-item"
            onClick={() => {
              onPick(t.id);
              onClose();
            }}
            style={{ alignItems: "flex-start", flexDirection: "column", gap: 2 }}
          >
            <span style={{ display: "flex", alignItems: "center", gap: 8 }}>
              <Icon name={t.icon} size={12} /> <span style={{ fontWeight: 500 }}>{t.label}</span>
            </span>
            <span style={{ fontSize: 11, color: "var(--fg-2)", lineHeight: 1.4, paddingLeft: 20 }}>
              {t.note}
            </span>
          </div>
        ))}
      </div>
    </>
  );
}

export function BlockSequence({
  blocks,
  seqNid,
  ctx,
}: {
  blocks: EBlock[];
  seqNid: NodeId | null;
  ctx: BuilderCtx;
}) {
  // The menu is fixed-positioned from the trigger's rect so it escapes any
  // overflow clip; `index` is where the picked block is inserted.
  const [menu, setMenu] = useState<{ top: number; left: number; index?: number } | null>(null);

  useDismissOnViewportChange(!!menu, () => setMenu(null));
  const canRemove = blocks.length > 1 || seqNid !== null;
  // A non-null seqNid means this is a loop body, and the engine executes loop
  // bodies of plain steps only (spec §6.6) — don't offer containers there.
  const blockTypes = seqNid === null ? BLOCK_TYPES : BLOCK_TYPES.filter((t) => t.id === "step");

  const openMenuAt = (e: React.MouseEvent, index?: number) => {
    const r = e.currentTarget.getBoundingClientRect();
    setMenu((m) => (m ? null : { top: r.bottom + 6, left: r.left, index }));
  };

  return (
    <div className="wb-seq">
      {blocks.map((b, i) => (
        <Fragment key={b.nid}>
          {i > 0 && (
            <div className="wb-conn">
              <button
                className="wb-insert tip"
                data-tip="Insert here"
                onClick={(e) => openMenuAt(e, i)}
              >
                <Icon name="plus" />
              </button>
            </div>
          )}
          <BlockNode block={b} index={i} ctx={ctx} canRemove={canRemove} />
        </Fragment>
      ))}

      <button className="wb-add" onClick={(e) => openMenuAt(e)}>
        <span className="wb-add-ic">
          <Icon name="plus" />
        </span>
        <span className="wb-add-l">{seqNid === null ? "Add block" : "Add step"}</span>
      </button>

      {menu && (
        <BlockMenu
          at={menu}
          types={blockTypes}
          onPick={(type) => ctx.addBlock(seqNid, type, menu.index)}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}
