/**
 * qr.ts regression test (GAP MAP v2, Part 5).
 *
 * The TOTP enrollment screen renders its QR with `lib/qr`, a dependency-free
 * encoder. This test locks the encoder to a known-good matrix so a future
 * edit cannot silently break scannability.
 *
 * The known-answer matrix below was produced by this encoder and verified two
 * ways at authoring time:
 *   1. bit-for-bit identical to the reference `qrcode` npm package output for
 *      the same payload at ECC level L (byte mode), and
 *   2. round-tripped through the independent `jsqr` decoder, which recovered
 *      the exact payload.
 *
 * Payload "hello world" (lowercase forces byte mode) -> version 1, mask 0.
 */
import { describe, expect, it } from "vitest";
import { encodeQr, qrSvgInner } from "@/lib/qr";

const KNOWN_PAYLOAD = "hello world";

const KNOWN_MATRIX = [
  "111111100101101111111",
  "100000100111001000001",
  "101110101101101011101",
  "101110100101001011101",
  "101110100010101011101",
  "100000100000101000001",
  "111111101010101111111",
  "000000001101100000000",
  "111011111111011000100",
  "000101011110001110011",
  "111011110100110111111",
  "010010010110000010010",
  "111010110010110110000",
  "000000001001010010111",
  "111111101001000110111",
  "100000101111100100001",
  "101110101011000010000",
  "101110100111001110110",
  "101110101100101010101",
  "100000101011000010010",
  "111111101101100100011",
];

describe("qr encoder", () => {
  it("matches the locked known-answer matrix for the reference payload", () => {
    const matrix = encodeQr(KNOWN_PAYLOAD);
    expect(matrix.length).toBe(KNOWN_MATRIX.length);
    const rendered = matrix.map((row) => row.map((v) => (v ? "1" : "0")).join(""));
    expect(rendered).toEqual(KNOWN_MATRIX);
  });

  it("produces a square matrix sized 17 + 4*version", () => {
    const cases: Array<[string, number]> = [
      ["a", 21], // version 1
      ["x".repeat(40), 29], // version 3
      ["x".repeat(100), 37], // version 5
    ];
    for (const [text, expected] of cases) {
      const matrix = encodeQr(text);
      expect(matrix.length).toBe(expected);
      for (const row of matrix) expect(row.length).toBe(expected);
    }
  });

  it("places the three finder patterns with a dark corner module", () => {
    const matrix = encodeQr(KNOWN_PAYLOAD);
    const n = matrix.length;
    const finderAt = (r: number, c: number) => {
      // 7x7 pattern: dark border, light ring, dark 3x3 core
      for (let dr = 0; dr < 7; dr += 1) {
        for (let dc = 0; dc < 7; dc += 1) {
          const border = dr === 0 || dr === 6 || dc === 0 || dc === 6;
          const core = dr >= 2 && dr <= 4 && dc >= 2 && dc <= 4;
          expect(matrix[r + dr]?.[c + dc]).toBe(border || core);
        }
      }
    };
    finderAt(0, 0);
    finderAt(0, n - 7);
    finderAt(n - 7, 0);
    // The always-dark module sits at (4*version + 9, 8); version 1 -> (13, 8).
    expect(matrix[13]?.[8]).toBe(true);
  });

  it("throws for payloads beyond version 9 level-L capacity", () => {
    expect(() => encodeQr("x".repeat(400))).toThrow(/too long/);
  });

  it("qrSvgInner emits a non-empty path sized to the matrix", () => {
    const matrix = encodeQr(KNOWN_PAYLOAD);
    const { size, path } = qrSvgInner(matrix);
    expect(size).toBe(matrix.length);
    expect(path.length).toBeGreaterThan(0);
    // every dark module becomes a unit-square path segment
    const darkCount = matrix.flat().filter(Boolean).length;
    expect(path.split("M").length - 1).toBe(darkCount);
  });
});
