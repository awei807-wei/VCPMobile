export function readStreamGeneration(
  event: any,
  context: Record<string, any>,
): number | null {
  const aliases = ["generation", "requestGeneration", "requestEpoch", "epoch"];
  const values = aliases.flatMap((key) => {
    const snake = key.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`);
    return [
      ...readPresentAliases(event, key, snake),
      ...readPresentAliases(context, key, snake),
    ];
  });
  if (values.length === 0) return null;
  if (values.some((value) => !isPositiveGeneration(value))) return null;
  const value = values[0];
  if (!values.every((candidate) => candidate === value)) return null;
  return isPositiveGeneration(value) ? value : null;
}

export function readPresentAliases(
  source: Record<string, any>,
  key: string,
  snake: string,
): unknown[] {
  return [key, snake]
    .filter((candidate, index, all) => all.indexOf(candidate) === index)
    .filter((candidate) => Object.prototype.hasOwnProperty.call(source, candidate))
    .map((candidate) => source[candidate]);
}

export function isPositiveGeneration(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value > 0;
}

export function hasInvalidRequiredAlias(
  event: Record<string, any>,
  context: Record<string, any>,
  keys: string[],
): boolean {
  const values = keys.flatMap((key) => {
    const snake = key.replace(/[A-Z]/g, (letter) => `_${letter.toLowerCase()}`);
    return [
      ...readPresentAliases(event, key, snake),
      ...readPresentAliases(context, key, snake),
    ];
  });
  return values.some(
    (value) => typeof value !== "string" || value.trim().length === 0,
  );
}
