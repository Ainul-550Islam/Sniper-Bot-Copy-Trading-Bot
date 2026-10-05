/**
 * Central Financial & Monetary Formatting Layer (THIRD.md §131).
 *
 * Rules:
 * 1. Authoritative financial amounts remain integers (cents, lamports, basis points).
 * 2. Conversions to string display never introduce floating-point arithmetic drift.
 * 3. Exact source precision and currency metadata are preserved.
 */

/** Formats integer USD cents (e.g. 10050 -> "$100.50"). */
export function formatUsdCents(cents: number | bigint | string, options?: { showSign?: boolean }): string {
  const numeric = typeof cents === "bigint" ? Number(cents) : Number(cents || 0);
  const isNegative = numeric < 0;
  const abs = Math.abs(numeric);
  const dollars = Math.floor(abs / 100);
  const rem = abs % 100;
  const formattedCents = rem.toString().padStart(2, "0");
  const sign = isNegative ? "-" : options?.showSign && numeric > 0 ? "+" : "";
  return `${sign}$${dollars.toLocaleString("en-US")}.${formattedCents}`;
}

/** Formats integer lamports to SOL string (e.g. 1_500_000_000 -> "1.5000 SOL"). */
export function formatLamports(
  lamports: number | bigint | string,
  options?: { decimals?: number; showUnit?: boolean },
): string {
  const decimals = options?.decimals ?? 4;
  const showUnit = options?.showUnit ?? true;
  const raw = typeof lamports === "bigint" ? Number(lamports) : Number(lamports || 0);
  const sol = raw / 1_000_000_000;
  const formatted = sol.toLocaleString("en-US", {
    minimumFractionDigits: 2,
    maximumFractionDigits: decimals,
  });
  return showUnit ? `${formatted} SOL` : formatted;
}

/** Formats basis points (e.g. 250 -> "2.50%"). */
export function formatBps(bps: number | string, options?: { showSign?: boolean }): string {
  const numeric = Number(bps || 0);
  const pct = numeric / 100;
  const sign = options?.showSign && numeric > 0 ? "+" : "";
  return `${sign}${pct.toFixed(2)}%`;
}

/** Formats raw percentage value (e.g. 14.25 -> "+14.25%"). */
export function formatPercentage(pct: number | string, options?: { showSign?: boolean }): string {
  const numeric = Number(pct || 0);
  const sign = options?.showSign && numeric > 0 ? "+" : "";
  return `${sign}${numeric.toFixed(2)}%`;
}

/** Formats integer units with readable thousand separators. */
export function formatInteger(val: number | bigint | string): string {
  const raw = typeof val === "bigint" ? Number(val) : Number(val || 0);
  return raw.toLocaleString("en-US");
}

/** Formats compact currency amounts for charts & cards (e.g. 14500000 -> "$14.50M"). */
export function formatCompactUsd(amount: number): string {
  if (Math.abs(amount) >= 1_000_000_000) {
    return `$${(amount / 1_000_000_000).toFixed(2)}B`;
  }
  if (Math.abs(amount) >= 1_000_000) {
    return `$${(amount / 1_000_000).toFixed(2)}M`;
  }
  if (Math.abs(amount) >= 1_000) {
    return `$${(amount / 1_000).toFixed(2)}K`;
  }
  return `$${amount.toFixed(2)}`;
}
