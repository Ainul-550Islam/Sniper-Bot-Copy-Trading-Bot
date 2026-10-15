import type { MetadataRoute } from "next";

import { branding } from "@/config/branding";

/**
 * Web app manifest (File 3 §7). Name, short name and theme colour come from
 * the same branding module as the rest of the chrome, so a white-label build
 * stays consistent. Icons are the logo shipped in /public.
 */
export default function manifest(): MetadataRoute.Manifest {
  return {
    name: branding.productName,
    short_name: branding.productName.slice(0, 12),
    description: branding.tagline,
    start_url: "/",
    display: "standalone",
    background_color: "#0b0d12",
    theme_color: branding.colors.primary,
    icons: [{ src: "/logo.svg", sizes: "any", type: "image/svg+xml" }],
  };
}
