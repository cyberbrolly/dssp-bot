export function parseTraineeInput(raw: string): string[] {
  const seen = new Set<string>();

  return raw
    .split(/[,\r\n]+/)
    .map((entry) => entry.trim())
    .filter((entry) => {
      if (!entry) return false;

      const key = entry.replace(/\s+/g, " ").toLocaleLowerCase();

      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
}
