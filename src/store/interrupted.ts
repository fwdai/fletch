// Agents the user just stopped. A killed turn may still flush a final `result`
// event (→ turn_end) as it dies; this set suppresses the completion chime for
// that one turn_end so a manual stop doesn't sound like a successful finish.
export const interruptedAgents = new Set<string>();
