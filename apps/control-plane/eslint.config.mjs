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
// Compiler-era `eslint-plugin-react-hooks` v7 rules). They are hard errors for
// every finding. Data-loader effects defer their initial async call to a
// microtask so the effect does not synchronously cascade state updates.
// No rule is disabled globally.
import nextCoreWebVitals from "eslint-config-next/core-web-vitals";
import nextTypescript from "eslint-config-next/typescript";

const config = [
  ...nextCoreWebVitals,
  ...nextTypescript,
  {
    files: ["src/**/*.{ts,tsx}"],
    rules: {
      "@typescript-eslint/no-explicit-any": "error",
      "react-hooks/set-state-in-effect": "error",
    },
  },
];

export default config;
