import type { NextConfig } from "next";
import { buildStaticSecurityHeaders } from "./src/lib/security-headers";

/**
 * The API origin is the ONLY thing this app reads from the environment.
 *
 * - unset            → same-origin (`""`): the Next server also fronts the
 *                      control-plane API (the usual embedded-dashboard-style
 *                      deployment).
 * - NEXT_PUBLIC_API_ORIGIN → an explicit origin for a split deployment.
 *
 * There is deliberately NO secret of any kind in client configuration:
 * signing keys, webhook secrets, the deployment key and the operator API key
 * never reach the browser in any form.
 */
const apiOrigin = process.env.NEXT_PUBLIC_API_ORIGIN ?? "";
const isProd = process.env.NODE_ENV === "production";

const nextConfig: NextConfig = {
  output: "standalone",
  reactStrictMode: true,
  poweredByHeader: false,
  // Linting is an explicit `npm run lint` step with the repository's shared
  // configuration (`eslint.config.mjs` + eslint-config-next) — never an
  // implicit, version-dependent build side effect. Next.js 16 removed both the
  // `next lint` command and the `eslint` key in this config, so the rule is now
  // enforced by the framework itself plus `.github/workflows/frontend-ci.yml`.
  env: {
    NEXT_PUBLIC_API_ORIGIN: apiOrigin,
  },
  async headers() {
    // Static headers only (XFO, nosniff, referrer, permissions policy, COOP,
    // CORP, HSTS in production). The Content-Security-Policy is per-request —
    // it carries a fresh nonce — and is emitted by `src/proxy.ts` instead; a
    // CSP built once at build time could only be a static, nonce-less policy.
    return [
      {
        source: "/:path*",
        headers: buildStaticSecurityHeaders({ isProd }),
      },
    ];
  },
};

export default nextConfig;
