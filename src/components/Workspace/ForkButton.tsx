import { ForkMenu, type ForkOption } from "./ForkMenu";

/** "Fork from here" affordance under an ended turn: forks a new workspace
 *  carrying the conversation up to this turn, with a choice of clean vs. current
 *  code. The token-slicing / branch-a-direction entry point. */
export function ForkButton({ agentId, upToPrompt }: { agentId: string; upToPrompt: number }) {
  const context = { kind: "up_to_message", prompt: upToPrompt } as const;
  const options: ForkOption[] = [
    {
      key: "clean",
      label: "Fork here · clean workspace",
      code: "clean",
      context,
    },
    {
      key: "carry",
      label: "Fork here · with current code",
      code: "carry",
      context,
    },
  ];
  return <ForkMenu agentId={agentId} options={options} tip="Fork to new workspace" compact />;
}
