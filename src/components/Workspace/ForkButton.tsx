import { ForkMenu, type ForkOption } from "./ForkMenu";

/** "Fork from here" affordance under an ended turn: forks a new workspace
 *  carrying the conversation through `turnId`, with a choice of clean vs.
 *  current code. The token-slicing / branch-a-direction entry point. */
export function ForkButton({ agentId, turnId }: { agentId: string; turnId: string }) {
  const options: ForkOption[] = [
    {
      key: "clean",
      label: "Fork here · clean workspace",
      code: "clean",
      context: "summary",
    },
    {
      key: "carry",
      label: "Fork here · with current code",
      code: "carry",
      context: "summary",
    },
  ];
  return (
    <ForkMenu
      agentId={agentId}
      turnId={turnId}
      options={options}
      tip="Fork to new workspace"
      compact
    />
  );
}
