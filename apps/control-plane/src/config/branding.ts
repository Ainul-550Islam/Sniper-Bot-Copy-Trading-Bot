/**
 * White-label branding (GAP-MAP v2, P2).
 *
 * Every customer-visible identity value lives here and is overridable at
 * build time through `NEXT_PUBLIC_*` environment variables, so a reseller
 * can ship the same control plane under their own name, logo, colours and
 * legal links without a code fork:
 *
 *   NEXT_PUBLIC_PRODUCT_NAME       product name in titles and chrome
 *   NEXT_PUBLIC_PRODUCT_TAGLINE    one-line tagline under the logo
 *   NEXT_PUBLIC_LOGO_URL           logo image URL (omit: text wordmark)
 *   NEXT_PUBLIC_BRAND_PRIMARY      primary accent colour (CSS colour)
 *   NEXT_PUBLIC_BRAND_ACCENT       secondary accent colour (CSS colour)
 *   NEXT_PUBLIC_SUPPORT_EMAIL      support contact shown in-app
 *   NEXT_PUBLIC_SUPPORT_URL        support/help-centre URL
 *   NEXT_PUBLIC_DOCS_URL           documentation URL
 *   NEXT_PUBLIC_LEGAL_TERMS_URL    terms of service
 *   NEXT_PUBLIC_LEGAL_PRIVACY_URL  privacy policy
 *   NEXT_PUBLIC_LEGAL_RISK_URL     trading risk disclosure
 *   NEXT_PUBLIC_LEGAL_DPA_URL      data processing agreement
 *
 * Rules:
 *  - this module is PURE configuration: it never fetches, never renders,
 *    and never fabricates. An unset value falls back to the compiled
 *    default below — defaults are the honest product identity, not
 *    placeholders;
 *  - colours are passed straight into CSS custom properties; invalid CSS
 *    simply degrades to inherited colours, it cannot break the page;
 *  - legal links are optional: a `null` link must be rendered as "not
 *    published", never as a dead href.
 */

export interface BrandLegalLinks {
  /** Terms of service URL, or null when not published. */
  readonly terms: string | null;
  /** Privacy policy URL, or null when not published. */
  readonly privacy: string | null;
  /** Trading risk disclosure URL, or null when not published. */
  readonly riskDisclosure: string | null;
  /** Data processing agreement URL, or null when not published. */
  readonly dpa: string | null;
}

export interface Branding {
  readonly productName: string;
  readonly tagline: string;
  /** Logo image URL; null = render the text wordmark instead. */
  readonly logoUrl: string | null;
  readonly colors: {
    readonly primary: string;
    readonly accent: string;
  };
  readonly supportEmail: string;
  /** Help-centre URL, or null when support is email-only. */
  readonly supportUrl: string | null;
  readonly docsUrl: string | null;
  readonly legalLinks: BrandLegalLinks;
}

/** Compiled product identity — the fallback for every unset variable. */
export const DEFAULT_BRANDING: Branding = Object.freeze({
  productName: "Sniper Suite",
  tagline: "Multi-venue trading control plane",
  logoUrl: null,
  colors: Object.freeze({
    primary: "#2f6fed",
    accent: "#22c55e",
  }),
  supportEmail: "support@snipersuite.example",
  supportUrl: null,
  docsUrl: null,
  legalLinks: Object.freeze({
    terms: "/legal/terms",
    privacy: "/legal/privacy",
    riskDisclosure: "/legal/risk-disclosure",
    dpa: null,
  }),
});

/**
 * Trim-to-null: whitespace-only env values count as unset (a blank
 * variable must fall back to the default, not become an empty label).
 */
function envOrNull(value: string | undefined): string | null {
  const trimmed = value?.trim();
  return trimmed && trimmed.length > 0 ? trimmed : null;
}

/**
 * Resolve the effective branding from the environment. Called once at
 * module load; environment variables are build-time constants in Next.js
 * (`NEXT_PUBLIC_*`), so the result is stable for the page's lifetime.
 */
export function resolveBranding(
  env: Record<string, string | undefined> = process.env,
): Branding {
  return Object.freeze({
    productName: envOrNull(env.NEXT_PUBLIC_PRODUCT_NAME) ?? DEFAULT_BRANDING.productName,
    tagline: envOrNull(env.NEXT_PUBLIC_PRODUCT_TAGLINE) ?? DEFAULT_BRANDING.tagline,
    logoUrl: envOrNull(env.NEXT_PUBLIC_LOGO_URL),
    colors: Object.freeze({
      primary: envOrNull(env.NEXT_PUBLIC_BRAND_PRIMARY) ?? DEFAULT_BRANDING.colors.primary,
      accent: envOrNull(env.NEXT_PUBLIC_BRAND_ACCENT) ?? DEFAULT_BRANDING.colors.accent,
    }),
    supportEmail:
      envOrNull(env.NEXT_PUBLIC_SUPPORT_EMAIL) ?? DEFAULT_BRANDING.supportEmail,
    supportUrl: envOrNull(env.NEXT_PUBLIC_SUPPORT_URL),
    docsUrl: envOrNull(env.NEXT_PUBLIC_DOCS_URL),
    legalLinks: Object.freeze({
      terms: envOrNull(env.NEXT_PUBLIC_LEGAL_TERMS_URL) ?? DEFAULT_BRANDING.legalLinks.terms,
      privacy:
        envOrNull(env.NEXT_PUBLIC_LEGAL_PRIVACY_URL) ??
        DEFAULT_BRANDING.legalLinks.privacy,
      riskDisclosure:
        envOrNull(env.NEXT_PUBLIC_LEGAL_RISK_URL) ??
        DEFAULT_BRANDING.legalLinks.riskDisclosure,
      dpa: envOrNull(env.NEXT_PUBLIC_LEGAL_DPA_URL),
    }),
  });
}

/** The branding used across the app (environment-resolved once). */
export const branding: Branding = resolveBranding();

/**
 * CSS custom-property assignments for the brand colours. Applied by the
 * root layout so components keep using `var(--brand-primary)` everywhere.
 */
export function brandCssVariables(b: Branding = branding): Record<string, string> {
  return {
    "--brand-primary": b.colors.primary,
    "--brand-accent": b.colors.accent,
  };
}
