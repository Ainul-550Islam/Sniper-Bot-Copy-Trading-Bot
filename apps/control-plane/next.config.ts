import type { NextConfig } from "next";
import { buildSecurityHeaders } from "./src/lib/security-headers";

/**
 * The API origin is the ONLY thing this app reads from the environment.
 *
 * - unset            → same-origin (`""`): the Next server also fronts the
 *                      control-plane API (the usual embedded-dashboard-style
 *                      deployment).
 * - NEXT_PUBLIC_API_ORIGIN → an explicit origin for a split deployment.
 *
 * There is deliberately NO secret of any kind in client configuration:
 * signing keys, webhook secrets and the deployment key never reach the
 * browser in any form.
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
    return [
      {
        source: "/:path*",
        headers: buildSecurityHeaders({ apiOrigin, isProd }),
      },
    ];
  },
};

export default nextConfig;
