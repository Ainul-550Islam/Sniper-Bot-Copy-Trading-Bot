import { describe, expect, it } from "vitest";
import { formatInteger, formatLamports, formatUsdCents } from "@/lib/formatters/financial";

describe("exact financial integer formatting", () => {
  it("formats large USD-cent bigint and integer-string values without losing cents", () => {
    expect(formatUsdCents(9007199254740993n)).toBe("$90,071,992,547,409.93");
    expect(formatUsdCents("9007199254740993")).toBe("$90,071,992,547,409.93");
    expect(formatUsdCents(-125n, { showSign: true })).toBe("-$1.25");
  });

  it("formats large lamport values exactly at requested source precision", () => {
    expect(formatLamports(9007199254740993123n, { decimals: 9 })).toBe(
      "9,007,199,254.740993123 SOL",
    );
    expect(formatLamports("100000000", { showUnit: false })).toBe("0.10");
  });

  it("formats large integer strings and bigints without Number precision loss", () => {
    expect(formatInteger(9007199254740993n)).toBe("9,007,199,254,740,993");
    expect(formatInteger("9007199254740993")).toBe("9,007,199,254,740,993");
  });
});
