/**
 * Vitest configuration for the control plane (GAP MAP v2, Part 5).
 *
 * The unit suite here is deliberately Node-environment: both contract tests
 * (`no-fake-data`, `api-contract`) are static analyses over `src/` and
 * `openapi/openapi.json` — they need the filesystem, not a DOM. If a future
 * component test needs a DOM, add `// @vitest-environment jsdom` to that
 * single file (and jsdom to devDependencies) rather than flipping the global
 * environment and slowing the scan tests down.
 *
 * Run with: npm run test:unit
 */
import { defineConfig } from "vitest/config";
import { fileURLToPath } from "node:url";

export default defineConfig({
  resolve: {
    alias: {
      // Same `@/` mapping as tsconfig.json so tests can import app modules.
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  test: {
    environment: "node",
    include: ["src/__tests__/**/*.test.ts"],
    // The scans walk the whole src/ tree; give CI boxes headroom.
    testTimeout: 30_000,
    reporters: "default",
  },
});
