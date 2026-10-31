/** Convert a nonnegative decimal exactly into integer base units, never by floats. */
export function units(value: string, decimals: number, label: string, allowZero = false): number {
  const match = /^(\d+)(?:\.(\d+))?$/.exec(value.trim());
  if (!match || (match[2]?.length ?? 0) > decimals) throw new Error(`${label} must have at most ${decimals} decimal places`);
  const amount = BigInt(match[1]!) * 10n ** BigInt(decimals) + BigInt((match[2] ?? "").padEnd(decimals, "0") || "0");
  if ((!allowZero && amount === 0n) || amount > BigInt(Number.MAX_SAFE_INTEGER)) throw new Error(`${label} is out of range`);
  return Number(amount);
}

/** Validate integer inputs without accepting scientific notation or truncation. */
export function whole(value: string, label: string, min: number, max: number): number {
  const normalized = value.trim();
  if (!/^\d+$/.test(normalized)) throw new Error(`${label} must be a whole number`);
  const parsed = Number(normalized);
  if (!Number.isSafeInteger(parsed) || parsed < min || parsed > max) throw new Error(`${label} is out of range`);
  return parsed;
}
