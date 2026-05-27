export function isRecord(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === "object" && !Array.isArray(v);
}

export function asRecord(v: unknown): Record<string, unknown> {
  return isRecord(v) ? v : {};
}

export function asBlockList(v: unknown): Array<Record<string, unknown>> {
  return Array.isArray(v) ? v.filter(isRecord) : [];
}
