/** Parst das capabilities-json-feld; leer = null, ungueltig = "invalid"-marker */
export function parseCapabilities(
  raw: string
): Record<string, unknown> | null | "invalid" {
  const trimmed = raw.trim();
  if (!trimmed) return null;
  try {
    const v = JSON.parse(trimmed);
    if (typeof v !== "object" || v === null || Array.isArray(v)) return "invalid";
    return v;
  } catch {
    return "invalid";
  }
}
