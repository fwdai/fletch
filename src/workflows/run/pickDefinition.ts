// run/pickDefinition.ts — which definition the workflow composer launches.
// Pure, so the precedence is testable: an explicit pick this session, else the
// project's remembered default, else the first definition. `remembered` is
// `undefined` while the project-settings read is still in flight, and then
// the answer is "not yet" — a run must never launch the first definition only
// because the remembered one hadn't arrived before the user pressed Enter.

export function pickDefinition<D extends { id: string }>(
  definitions: D[],
  picked: string,
  remembered: string | null | undefined,
): D | undefined {
  if (picked) return definitions.find((d) => d.id === picked) ?? definitions[0];
  if (remembered === undefined) return undefined;
  return definitions.find((d) => d.id === remembered) ?? definitions[0];
}
