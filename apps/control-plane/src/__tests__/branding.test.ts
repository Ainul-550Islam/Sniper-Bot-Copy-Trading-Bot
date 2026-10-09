/**
 * Branding white-label config (GAP-MAP v2, P2).
 *
 * The module must be pure: defaults are the honest product identity, every
 * value is overridable via NEXT_PUBLIC_* variables, and blank/whitespace
 * values fall back to the default (never become empty labels or dead hrefs).
 */
import { describe, expect, it } from "vitest";

import {
  DEFAULT_BRANDING,
  brandCssVariables,
  resolveBranding,
  type Branding,
} from "../config/branding";

describe("branding defaults", () => {
  it("ships a complete, non-empty default identity", () => {
    expect(DEFAULT_BRANDING.productName.length).toBeGreaterThan(0);
    expect(DEFAULT_BRANDING.tagline.length).toBeGreaterThan(0);
    expect(DEFAULT_BRANDING.supportEmail).toContain("@");
    expect(DEFAULT_BRANDING.colors.primary).toMatch(/^#/);
    expect(DEFAULT_BRANDING.colors.accent).toMatch(/^#/);
  });

  it("defaults legal links to real in-app pages or null", () => {
    const links = DEFAULT_BRANDING.legalLinks;
    expect(links.terms).toBe("/legal/terms");
    expect(links.privacy).toBe("/legal/privacy");
    expect(links.riskDisclosure).toBe("/legal/risk-disclosure");
    // DPA is genuinely unpublished by default — null, not a fake href.
    expect(links.dpa).toBeNull();
  });

  it("with an empty environment the resolution equals the defaults", () => {
    expect(resolveBranding({})).toEqual(DEFAULT_BRANDING);
  });
});

describe("branding environment overrides", () => {
  it("overrides product identity fields", () => {
    const b: Branding = resolveBranding({
      NEXT_PUBLIC_PRODUCT_NAME: "Acme Trader",
      NEXT_PUBLIC_PRODUCT_TAGLINE: "Trade like Acme",
      NEXT_PUBLIC_LOGO_URL: "https://cdn.acme.example/logo.svg",
      NEXT_PUBLIC_SUPPORT_EMAIL: "help@acme.example",
    });
    expect(b.productName).toBe("Acme Trader");
    expect(b.tagline).toBe("Trade like Acme");
    expect(b.logoUrl).toBe("https://cdn.acme.example/logo.svg");
    expect(b.supportEmail).toBe("help@acme.example");
    // Untouched fields keep defaults.
    expect(b.colors.primary).toBe(DEFAULT_BRANDING.colors.primary);
  });

  it("overrides colours and legal links", () => {
    const b = resolveBranding({
      NEXT_PUBLIC_BRAND_PRIMARY: "#ff0000",
      NEXT_PUBLIC_BRAND_ACCENT: "#00ff00",
      NEXT_PUBLIC_LEGAL_TERMS_URL: "https://acme.example/terms",
      NEXT_PUBLIC_LEGAL_PRIVACY_URL: "https://acme.example/privacy",
      NEXT_PUBLIC_LEGAL_RISK_URL: "https://acme.example/risk",
      NEXT_PUBLIC_LEGAL_DPA_URL: "https://acme.example/dpa",
    });
    expect(b.colors).toEqual({ primary: "#ff0000", accent: "#00ff00" });
    expect(b.legalLinks.terms).toBe("https://acme.example/terms");
    expect(b.legalLinks.privacy).toBe("https://acme.example/privacy");
    expect(b.legalLinks.riskDisclosure).toBe("https://acme.example/risk");
    expect(b.legalLinks.dpa).toBe("https://acme.example/dpa");
  });

  it("treats whitespace-only values as unset (falls back to defaults)", () => {
    const b = resolveBranding({
      NEXT_PUBLIC_PRODUCT_NAME: "   ",
      NEXT_PUBLIC_SUPPORT_EMAIL: "\t",
      NEXT_PUBLIC_LOGO_URL: "",
    });
    expect(b.productName).toBe(DEFAULT_BRANDING.productName);
    expect(b.supportEmail).toBe(DEFAULT_BRANDING.supportEmail);
    expect(b.logoUrl).toBeNull();
  });

  it("omitted optional values resolve to null, never empty strings", () => {
    const b = resolveBranding({});
    expect(b.logoUrl).toBeNull();
    expect(b.supportUrl).toBeNull();
    expect(b.docsUrl).toBeNull();
    expect(b.legalLinks.dpa).toBeNull();
  });

  it("trims surrounding whitespace from real values", () => {
    const b = resolveBranding({ NEXT_PUBLIC_PRODUCT_NAME: "  Padded Name  " });
    expect(b.productName).toBe("Padded Name");
  });
});

describe("brandCssVariables", () => {
  it("maps the brand colours to CSS custom properties", () => {
    const vars = brandCssVariables(DEFAULT_BRANDING);
    expect(vars["--brand-primary"]).toBe(DEFAULT_BRANDING.colors.primary);
    expect(vars["--brand-accent"]).toBe(DEFAULT_BRANDING.colors.accent);
  });

  it("reflects overridden colours", () => {
    const b = resolveBranding({ NEXT_PUBLIC_BRAND_PRIMARY: "#123456" });
    expect(brandCssVariables(b)["--brand-primary"]).toBe("#123456");
  });
});
