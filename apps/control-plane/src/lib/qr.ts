/**
 * Minimal, dependency-free QR encoder (byte mode, ECC level L, versions 1-9).
 *
 * Why this exists: the TOTP enrollment screen must render the `otpauth://`
 * URL as a scannable QR code. The control plane ships with zero runtime
 * dependencies for this (no CDN fetch — the preview iframe has no network,
 * and an auth surface should never phone home), so we implement the subset
 * of ISO/IEC 18004 we actually need:
 *
 *   - byte-mode encoding (the otpauth URL is ASCII),
 *   - error-correction level L (the payload is small and re-typable; the
 *     secret is also shown as text beside the code),
 *   - versions 1-9 (up to 230 payload bytes — an otpauth URL is ~110),
 *   - full mask evaluation (all eight masks, all four penalty rules),
 *     because authenticator apps are picky scanners.
 *
 * The implementation is validated against the reference `qrcode` npm package
 * in a one-off harness (matrices must match bit-for-bit, mask included); if
 * you change anything here, re-run that cross-check.
 */

/** ECC level L capacity table, versions 1..9. */
interface VersionInfo {
  /** total data codewords at level L */
  dataCodewords: number;
  /** error-correction codewords per block at level L */
  ecPerBlock: number;
  /** number of blocks at level L (equal-size blocks only) */
  blocks: number;
  /** alignment-pattern center coordinates */
  alignment: number[];
}

const VERSIONS: VersionInfo[] = [
  { dataCodewords: 0, ecPerBlock: 0, blocks: 0, alignment: [] }, // index 0 unused
  { dataCodewords: 19, ecPerBlock: 7, blocks: 1, alignment: [] },
  { dataCodewords: 34, ecPerBlock: 10, blocks: 1, alignment: [6, 18] },
  { dataCodewords: 55, ecPerBlock: 15, blocks: 1, alignment: [6, 22] },
  { dataCodewords: 80, ecPerBlock: 20, blocks: 1, alignment: [6, 26] },
  { dataCodewords: 108, ecPerBlock: 26, blocks: 1, alignment: [6, 30] },
  { dataCodewords: 136, ecPerBlock: 18, blocks: 2, alignment: [6, 34] },
  { dataCodewords: 156, ecPerBlock: 20, blocks: 2, alignment: [6, 22, 38] },
  { dataCodewords: 194, ecPerBlock: 24, blocks: 2, alignment: [6, 24, 42] },
  { dataCodewords: 232, ecPerBlock: 30, blocks: 2, alignment: [6, 26, 46] },
];

/** Galois field tables for GF(256) with the QR polynomial 0x11d. */
const GF_EXP = new Uint8Array(512);
const GF_LOG = new Uint8Array(256);
{
  let value = 1;
  for (let i = 0; i < 255; i += 1) {
    GF_EXP[i] = value;
    GF_LOG[value] = i;
    value <<= 1;
    if (value & 0x100) value ^= 0x11d;
  }
  for (let i = 255; i < 512; i += 1) GF_EXP[i] = GF_EXP[i - 255] ?? 0;
}

function gfMul(a: number, b: number): number {
  if (a === 0 || b === 0) return 0;
  return GF_EXP[(GF_LOG[a] ?? 0) + (GF_LOG[b] ?? 0)] ?? 0;
}

/** Version table lookup that narrows away `undefined` for strict TS. */
function versionInfo(version: number): VersionInfo {
  const info = VERSIONS[version];
  if (!info) throw new Error(`qr: unsupported version ${version}`);
  return info;
}

/** Generator polynomial of degree `degree` (coefficients, highest first). */
function rsGenerator(degree: number): Uint8Array {
  let poly = new Uint8Array([1]);
  for (let i = 0; i < degree; i += 1) {
    const next = new Uint8Array(poly.length + 1);
    for (let j = 0; j < poly.length; j += 1) {
      next[j] = (next[j] ?? 0) ^ (poly[j] ?? 0);
      next[j + 1] = (next[j + 1] ?? 0) ^ gfMul(poly[j] ?? 0, GF_EXP[i] ?? 0);
    }
    poly = next;
  }
  return poly;
}

/** Reed-Solomon error-correction codewords for `data`. */
function rsEncode(data: Uint8Array, ecCount: number): Uint8Array {
  const generator = rsGenerator(ecCount);
  const remainder = new Uint8Array(ecCount);
  for (const byte of data) {
    const factor = byte ^ (remainder[0] ?? 0);
    remainder.copyWithin(0, 1);
    remainder[ecCount - 1] = 0;
    if (factor !== 0) {
      for (let i = 0; i < ecCount; i += 1) {
        remainder[i] = (remainder[i] ?? 0) ^ gfMul(generator[i + 1] ?? 0, factor);
      }
    }
  }
  return remainder;
}

/** Choose the smallest version whose byte-mode capacity fits `length`. */
function pickVersion(length: number): number {
  for (let version = 1; version <= 9; version += 1) {
    // 4 mode bits + 8 count bits (versions 1-9) are the fixed overhead.
    const capacity = versionInfo(version).dataCodewords - 2;
    if (length <= capacity) return version;
  }
  throw new Error("qr: payload too long for versions 1-9 (level L)");
}

/** Serialize the payload into codewords (data + interleaved ECC). */
function toCodewords(payload: Uint8Array, version: number): Uint8Array {
  const info = versionInfo(version);
  const totalData = info.dataCodewords;
  const bits: number[] = [];
  const push = (value: number, count: number) => {
    for (let i = count - 1; i >= 0; i -= 1) bits.push((value >> i) & 1);
  };
  push(0b0100, 4); // byte mode
  push(payload.length, 8); // count (versions 1-9 use 8 bits)
  for (const byte of payload) push(byte, 8);
  // terminator (up to 4 zero bits)
  const capacityBits = totalData * 8;
  const terminator = Math.min(4, capacityBits - bits.length);
  push(0, terminator);
  // pad to a byte boundary
  while (bits.length % 8 !== 0) bits.push(0);
  const data = new Uint8Array(totalData);
  const dataBytes = Math.ceil(bits.length / 8);
  for (let i = 0; i < dataBytes; i += 1) {
    let byte = 0;
    for (let b = 0; b < 8; b += 1) byte = (byte << 1) | (bits[i * 8 + b] ?? 0);
    data[i] = byte;
  }
  // alternating pad codewords 0xec / 0x11 fill the remainder
  for (let i = dataBytes; i < totalData; i += 1) {
    data[i] = (i - dataBytes) % 2 === 0 ? 0xec : 0x11;
  }

  // Split into blocks, compute ECC, interleave.
  const blockCount = info.blocks;
  const blockSize = Math.floor(totalData / blockCount);
  const dataBlocks: Uint8Array[] = [];
  const ecBlocks: Uint8Array[] = [];
  for (let b = 0; b < blockCount; b += 1) {
    const block = data.subarray(b * blockSize, (b + 1) * blockSize);
    dataBlocks.push(block);
    ecBlocks.push(rsEncode(block, info.ecPerBlock));
  }
  const out = new Uint8Array(totalData + blockCount * info.ecPerBlock);
  let offset = 0;
  for (let i = 0; i < blockSize; i += 1) {
    for (const block of dataBlocks) out[offset++] = block[i] ?? 0;
  }
  for (let i = 0; i < info.ecPerBlock; i += 1) {
    for (const block of ecBlocks) out[offset++] = block[i] ?? 0;
  }
  return out;
}

/**
 * Module grid bookkeeping: `modules` holds 0/1 values, `isFunction` marks
 * function/reserved modules that masking must not touch.
 */
class Matrix {
  readonly size: number;
  readonly modules: Uint8Array;
  readonly isFunction: Uint8Array;

  constructor(version: number) {
    this.size = 17 + 4 * version;
    this.modules = new Uint8Array(this.size * this.size);
    this.isFunction = new Uint8Array(this.size * this.size);
  }

  index(row: number, col: number): number {
    return row * this.size + col;
  }

  set(row: number, col: number, value: boolean, fn = false): void {
    this.modules[this.index(row, col)] = value ? 1 : 0;
    if (fn) this.isFunction[this.index(row, col)] = 1;
  }

  get(row: number, col: number): boolean {
    return this.modules[this.index(row, col)] === 1;
  }
}

function drawFunctionPatterns(matrix: Matrix, version: number): void {
  const size = matrix.size;
  const info = versionInfo(version);

  // Timing patterns.
  for (let i = 0; i < size; i += 1) {
    const dark = i % 2 === 0;
    matrix.set(6, i, dark, true);
    matrix.set(i, 6, dark, true);
  }

  // Finder patterns with separators.
  const drawFinder = (row: number, col: number) => {
    for (let dr = -1; dr <= 7; dr += 1) {
      for (let dc = -1; dc <= 7; dc += 1) {
        const r = row + dr;
        const c = col + dc;
        if (r < 0 || r >= size || c < 0 || c >= size) continue;
        const inRing =
          dr >= 0 && dr <= 6 && dc >= 0 && dc <= 6 &&
          (dr === 0 || dr === 6 || dc === 0 || dc === 6 ||
            (dr >= 2 && dr <= 4 && dc >= 2 && dc <= 4));
        matrix.set(r, c, inRing, true);
      }
    }
  };
  drawFinder(0, 0);
  drawFinder(0, size - 7);
  drawFinder(size - 7, 0);

  // Alignment patterns.
  const centers = info.alignment;
  for (const row of centers) {
    for (const col of centers) {
      // Skip the three corners occupied by finder patterns.
      if (
        (row === centers[0] && col === centers[0]) ||
        (row === centers[0] && col === centers[centers.length - 1]) ||
        (row === centers[centers.length - 1] && col === centers[0])
      ) {
        continue;
      }
      for (let dr = -2; dr <= 2; dr += 1) {
        for (let dc = -2; dc <= 2; dc += 1) {
          const dark = Math.max(Math.abs(dr), Math.abs(dc)) !== 1;
          matrix.set(row + dr, col + dc, dark, true);
        }
      }
    }
  }

  // Reserve format-info areas (values written after masking). i === 6 cells
  // belong to the timing patterns and stay dark — the format info never
  // uses them.
  for (let i = 0; i < 8; i += 1) {
    if (i !== 6) {
      matrix.set(8, i, false, true);
      matrix.set(i, 8, false, true);
    }
    matrix.set(8, size - 1 - i, false, true);
    matrix.set(size - 1 - i, 8, false, true);
  }
  matrix.set(8, 8, false, true);
  matrix.set(size - 8, 8, true, true); // the always-dark module

  // Reserve version-info areas (versions >= 7).
  if (version >= 7) {
    for (let i = 0; i < 18; i += 1) {
      const row = Math.floor(i / 3);
      const col = size - 11 + (i % 3);
      matrix.set(row, col, false, true);
      matrix.set(col, row, false, true);
    }
  }
}

/** Zigzag data placement; returns the bit positions in placement order. */
function dataModulePositions(matrix: Matrix): Array<[number, number]> {
  const size = matrix.size;
  const positions: Array<[number, number]> = [];
  let upward = true;
  for (let right = size - 1; right >= 1; right -= 2) {
    if (right === 6) right = 5; // skip the vertical timing column entirely
    const rows = upward
      ? Array.from({ length: size }, (_, i) => size - 1 - i)
      : Array.from({ length: size }, (_, i) => i);
    for (const row of rows) {
      for (const col of [right, right - 1]) {
        if (!matrix.isFunction[matrix.index(row, col)]) {
          positions.push([row, col]);
        }
      }
    }
    upward = !upward;
  }
  return positions;
}

function applyMask(matrix: Matrix, positions: Array<[number, number]>, mask: number): void {
  const test = (row: number, col: number): boolean => {
    switch (mask) {
      case 0: return (row + col) % 2 === 0;
      case 1: return row % 2 === 0;
      case 2: return col % 3 === 0;
      case 3: return (row + col) % 3 === 0;
      case 4: return (Math.floor(row / 2) + Math.floor(col / 3)) % 2 === 0;
      case 5: return ((row * col) % 2) + ((row * col) % 3) === 0;
      case 6: return (((row * col) % 2) + ((row * col) % 3)) % 2 === 0;
      default: return (((row + col) % 2) + ((row * col) % 3)) % 2 === 0;
    }
  };
  for (const [row, col] of positions) {
    if (test(row, col)) {
      const idx = matrix.index(row, col);
      matrix.modules[idx] = (matrix.modules[idx] ?? 0) ^ 1;
    }
  }
}

/** BCH(15,5) format info: 5 data bits -> 15 coded bits, XOR mask 0x5412. */
function formatBits(eccLevelBits: number, mask: number): number {
  const data = (eccLevelBits << 3) | mask;
  let value = data << 10;
  const generator = 0b10100110111; // x^10 + x^8 + x^5 + x^4 + x^2 + x + 1
  for (let i = 14; i >= 10; i -= 1) {
    if ((value >> i) & 1) value ^= generator << (i - 10);
  }
  return ((data << 10) | value) ^ 0b101010000010010;
}

/** BCH(18,6) version info for versions >= 7. */
function versionBits(version: number): number {
  let value = version << 12;
  const generator = 0b1111100100101; // x^12 + ... (the 0x1f25 polynomial)
  for (let i = 17; i >= 12; i -= 1) {
    if ((value >> i) & 1) value ^= generator << (i - 12);
  }
  return (version << 12) | value;
}

function drawFormatAndVersion(matrix: Matrix, version: number, mask: number): void {
  const size = matrix.size;
  // ECC level L == 0b01. `bits` is the 15-bit coded value; `bit(n)` reads
  // bit n (0 = LSB).
  const bits = formatBits(0b01, mask);
  const bit = (n: number) => ((bits >> n) & 1) === 1;
  // Copy 1 around the top-left finder, MSB first: (8,0..5) = bits 14..9,
  // then (8,7)=bit8, (8,8)=bit7, (7,8)=bit6, (5..0,8)=bits 5..0.
  for (let k = 0; k < 6; k += 1) {
    matrix.set(8, k, bit(14 - k), true);
    matrix.set(5 - k, 8, bit(5 - k), true);
  }
  matrix.set(8, 7, bit(8), true);
  matrix.set(8, 8, bit(7), true);
  matrix.set(7, 8, bit(6), true);
  // Copy 2: bottom-left column rows size-7..size-1 = bits 8..14 (ascending
  // downwards), top-right row (8, size-8..size-1) = bits 7..0 (descending).
  // Row size-8 column 8 stays the always-dark module.
  for (let k = 0; k < 7; k += 1) matrix.set(size - 7 + k, 8, bit(8 + k), true);
  for (let k = 0; k < 8; k += 1) matrix.set(8, size - 8 + k, bit(7 - k), true);
  if (version >= 7) {
    const vbits = versionBits(version);
    for (let i = 0; i < 18; i += 1) {
      const bit = ((vbits >> i) & 1) === 1;
      const row = Math.floor(i / 3);
      const col = size - 11 + (i % 3);
      matrix.set(row, col, bit, true);
      matrix.set(col, row, bit, true);
    }
  }
}

/** The four penalty rules from ISO 18004 section 8.8. */
function penalty(matrix: Matrix): number {
  const size = matrix.size;
  let score = 0;
  const run = (cells: boolean[]) => {
    let runLen = 1;
    for (let i = 1; i <= cells.length; i += 1) {
      if (i < cells.length && cells[i] === cells[i - 1]) {
        runLen += 1;
      } else {
        if (runLen >= 5) score += 3 + (runLen - 5);
        runLen = 1;
      }
    }
  };
  for (let row = 0; row < size; row += 1) {
    run(Array.from({ length: size }, (_, col) => matrix.get(row, col)));
  }
  for (let col = 0; col < size; col += 1) {
    run(Array.from({ length: size }, (_, row) => matrix.get(row, col)));
  }
  // Rule 2: 2x2 blocks.
  for (let row = 0; row < size - 1; row += 1) {
    for (let col = 0; col < size - 1; col += 1) {
      const v = matrix.get(row, col);
      if (v === matrix.get(row, col + 1) && v === matrix.get(row + 1, col) && v === matrix.get(row + 1, col + 1)) {
        score += 3;
      }
    }
  }
  // Rule 3: finder-like 1:1:3:1:1 patterns with 4 light modules either side.
  const pattern1 = [true, false, true, true, true, false, true, false, false, false, false];
  const pattern2 = [...pattern1].reverse();
  const matches = (cells: boolean[], start: number, pattern: boolean[]) =>
    pattern.every((p, i) => cells[start + i] === p);
  for (let row = 0; row < size; row += 1) {
    const cells = Array.from({ length: size }, (_, col) => matrix.get(row, col));
    for (let col = 0; col + 11 <= size; col += 1) {
      if (matches(cells, col, pattern1) || matches(cells, col, pattern2)) score += 40;
    }
  }
  for (let col = 0; col < size; col += 1) {
    const cells = Array.from({ length: size }, (_, row) => matrix.get(row, col));
    for (let row = 0; row + 11 <= size; row += 1) {
      if (matches(cells, row, pattern1) || matches(cells, row, pattern2)) score += 40;
    }
  }
  // Rule 4: dark ratio.
  let dark = 0;
  for (let i = 0; i < matrix.modules.length; i += 1) dark += matrix.modules[i] ?? 0;
  const percent = (dark * 100) / matrix.modules.length;
  score += Math.floor(Math.abs(percent - 50) / 5) * 10;
  return score;
}

/** Encode `text` and return the module matrix (true = dark). */
export function encodeQr(text: string): boolean[][] {
  const payload = new TextEncoder().encode(text);
  const version = pickVersion(payload.length);
  const codewords = toCodewords(payload, version);

  const bits: number[] = [];
  for (const byte of codewords) {
    for (let i = 7; i >= 0; i -= 1) bits.push((byte >> i) & 1);
  }

  let best: { matrix: Matrix; mask: number; score: number } | null = null;
  for (let mask = 0; mask < 8; mask += 1) {
    const matrix = new Matrix(version);
    drawFunctionPatterns(matrix, version);
    const positions = dataModulePositions(matrix);
    positions.forEach(([row, col], i) => {
      matrix.set(row, col, bits[i] === 1);
    });
    applyMask(matrix, positions, mask);
    drawFormatAndVersion(matrix, version, mask);
    const score = penalty(matrix);
    if (best === null || score < best.score) best = { matrix, mask, score };
  }

  if (!best) throw new Error("qr: no mask candidate evaluated");
  const { matrix } = best;
  const rows: boolean[][] = [];
  for (let row = 0; row < matrix.size; row += 1) {
    rows.push(Array.from({ length: matrix.size }, (_, col) => matrix.get(row, col)));
  }
  return rows;
}

/**
 * Render the matrix as SVG path text (one `<path>` with unit squares) for a
 * compact, dependency-free inline SVG. `scale` is the module size in px.
 */
export function qrSvgInner(qr: boolean[][]): { size: number; path: string } {
  const size = qr.length;
  const parts: string[] = [];
  for (let row = 0; row < size; row += 1) {
    for (let col = 0; col < size; col += 1) {
      if (qr[row]?.[col]) parts.push(`M${col} ${row}h1v1h-1z`);
    }
  }
  return { size, path: parts.join("") };
}
