import type { Metadata, Viewport } from "next";
import "@/styles/globals.css";

/**
 * The control plane's root layout (TASK 7B file 04).
 *
 * Global styles, document metadata and the favicon live here and only here;
 * nothing session-related happens during server rendering — the session is
 * established client-side (see `src/app/page.tsx`), because the credential
 * never leaves the browser's memory.
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

export default function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
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
