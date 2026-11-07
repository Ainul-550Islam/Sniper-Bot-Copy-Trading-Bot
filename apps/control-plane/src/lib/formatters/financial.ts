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
  const integerString = typeof cents === "string" && /^[+-]?\d+$/.test(cents.trim());
  if (typeof cents === "bigint" || integerString) {
    const exact = typeof cents === "bigint" ? cents : BigInt(cents.trim());
    const isNegative = exact < 0n;
    const abs = isNegative ? -exact : exact;
    const dollars = abs / 100n;
    const rem = (abs % 100n).toString().padStart(2, "0");
    const sign = isNegative ? "-" : options?.showSign && exact > 0n ? "+" : "";
    return `${sign}$${dollars.toLocaleString("en-US")}.${rem}`;
  }

  const numeric = Number(cents || 0);
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
  const integerString = typeof lamports === "string" && /^[+-]?\d+$/.test(lamports.trim());
  if (typeof lamports === "bigint" || integerString) {
    const exact = typeof lamports === "bigint" ? lamports : BigInt(lamports.trim());
    // Keep Intl's option validation and its minimum/maximum fraction rules.
    new Intl.NumberFormat("en-US", { minimumFractionDigits: 2, maximumFractionDigits: decimals });

    const negative = exact < 0n;
    const absolute = negative ? -exact : exact;
    const nanoSol = 1_000_000_000n;
    let whole: bigint;
    let fraction: string;
    if (decimals <= 9) {
      const scale = 10n ** BigInt(decimals);
      const divisor = nanoSol / scale;
      let rounded = absolute / divisor;
      if ((absolute % divisor) * 2n >= divisor) rounded += 1n;
      whole = rounded / scale;
      fraction = (rounded % scale).toString().padStart(decimals, "0");
    } else {
      whole = absolute / nanoSol;
      fraction = (absolute % nanoSol).toString().padStart(9, "0") + "0".repeat(decimals - 9);
    }
    while (fraction.length > 2 && fraction.endsWith("0")) fraction = fraction.slice(0, -1);
    const formatted = `${negative ? "-" : ""}${whole.toLocaleString("en-US")}.${fraction}`;
    return showUnit ? `${formatted} SOL` : formatted;
  }

  const raw = Number(lamports || 0);
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
  const integerString = typeof val === "string" && /^[+-]?\d+$/.test(val.trim());
  if (typeof val === "bigint" || integerString) {
    return (typeof val === "bigint" ? val : BigInt(val.trim())).toLocaleString("en-US");
  }
  const raw = Number(val || 0);
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
