import type { Metadata, Viewport } from "next";
import { headers } from "next/headers";
import { CSP_NONCE_HEADER } from "@/lib/security-headers";
import "@/styles/globals.css";

/**
 * The control plane's root layout (TASK 7B file 04).
 *
 * Global styles, document metadata and the favicon live here and only here;
 * nothing session-related happens during server rendering — the session is
 * established client-side (see `src/app/page.tsx`), because the credential
 * never leaves the browser's memory.
 *
 * The one server-side read is the per-request CSP nonce: `src/proxy.ts` mints
 * it, Next.js re-reads it off the response CSP header and stamps every
 * framework script (including the inline flight-data scripts) with it, and this
 * layout exposes the same value so any inline `<script>` added to this document
 * carries `nonce={cspNonce}` — without it the script is blocked, because
 * `script-src` has no `'unsafe-inline'`.
 */
export const metadata: Metadata = {
  title: {
    default: "Sniper Suite — Control Plane",
    template: "%s · Sniper Suite",
  },
  description:
    "Multi-tenant control plane for the Sniper Suite trading system: tenants, wallets, billing, usage and audit. Trading truth stays in the operator engines.",
  icons: { icon: "/logo.svg" },
  robots: { index: false, follow: false },
};

export const viewport: Viewport = {
  width: "device-width",
  initialScale: 1,
};

export default async function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  // Awaiting headers() is what pins this document to the request that owns the
  // nonce; a statically rendered layout could not carry one.
  const cspNonce = (await headers()).get(CSP_NONCE_HEADER);
  // There is no inline <script> in this document today. The nonce is read so
  // that adding one is nonced by construction rather than discovered as a
  // production breakage; see src/__tests__/security-headers.test.ts and
  // src/__tests__/proxy.test.ts for the per-request nonce contract.
  void cspNonce;

  return (
    <html lang="en">
      <body>
        <a href="#main" className="skip-link">
          Skip to content
        </a>
        {children}
      </body>
    </html>
  );
}
