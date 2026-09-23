import type { NextConfig } from "next";

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

const nextConfig: NextConfig = {
  reactStrictMode: true,
  poweredByHeader: false,
  // Linting is an explicit `npm run lint` step with the repository's shared
  // configuration — never an implicit, version-dependent build side effect.
  eslint: { ignoreDuringBuilds: true },
  env: {
    NEXT_PUBLIC_API_ORIGIN: apiOrigin,
  },
  async headers() {
    return [
      {
        // Belt-and-braces: the backend already sends these on every
        // response; the Next layer repeats the essentials for static assets.
        source: "/:path*",
        headers: [
          { key: "X-Content-Type-Options", value: "nosniff" },
          { key: "X-Frame-Options", value: "DENY" },
          { key: "Referrer-Policy", value: "strict-origin-when-cross-origin" },
        ],
      },
    ];
  },
};

export default nextConfig;
