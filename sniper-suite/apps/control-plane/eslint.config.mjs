// Flat ESLint configuration (ESLint 9) for the control-plane frontend.
//
// Why this file exists: Next.js 16 removed the `next lint` command and the
// `eslint` key in `next.config.ts`; linting is ESLint's own CLI from now on
// (https://nextjs.org/docs/app/api-reference/config/eslint). This configuration
// keeps the repository's shared rule source — `eslint-config-next`, already a
// devDependency — so `npm run lint` stays non-interactive for
// `.github/workflows/frontend-ci.yml` and for buyers.
//
// Rule level (documented, not silent): the two rule families below arrive with
// `eslint-config-next` 16 (typescript-eslint 8 "recommended" and the React
// Compiler-era `eslint-plugin-react-hooks` v7 rules). The frontend in this
// repository predates both, so the pre-existing findings are reported as
// warnings — they stay visible in every lint run and are enumerated in
// `docs/KNOWN-LIMITATIONS.md` (row 16) — while anything new still surfaces on
// the same run. No rule is disabled outright.
import nextCoreWebVitals from "eslint-config-next/core-web-vitals";
import nextTypescript from "eslint-config-next/typescript";

const config = [
  ...nextCoreWebVitals,
  ...nextTypescript,
  {
    files: ["src/**/*.{ts,tsx}"],
    rules: {
      // 7 pre-existing sites (custody page / data-lifecycle page) parse
      // dynamic provider payloads with `as any` narrowing. Typing them properly
      // is a frontend task, tracked in the limitations register.
      "@typescript-eslint/no-explicit-any": "warn",
      // 3 pre-existing data-loading effects set loading state synchronously
      // (AppShell ×2, data-lifecycle ×1). Restructuring them is a behaviour
      // change and is deliberately out of scope for a dependency remediation.
      "react-hooks/set-state-in-effect": "warn",
    },
  },
];

export default config;
