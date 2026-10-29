import { describe, expect, it } from "vitest";
import { buildSecurityHeaders } from "../lib/security-headers";

describe("security-headers", () => {
  it("asserts CSP has frame-ancestors 'none'", () => {
    const headers = buildSecurityHeaders({ isProd: true });
    const csp = headers.find((h) => h.key === "Content-Security-Policy");
    expect(csp).toBeDefined();
    expect(csp!.value).toContain("frame-ancestors 'none'");
    expect(csp!.value).toContain("default-src 'self'");
    expect(csp!.value).toContain("object-src 'none'");
    expect(csp!.value).toContain("base-uri 'self'");
    expect(csp!.value).toContain("form-action 'self'");
  });

  it("asserts no 'unsafe-eval' in prod", () => {
    const headers = buildSecurityHeaders({ isProd: true });
    const csp = headers.find((h) => h.key === "Content-Security-Policy");
    expect(csp).toBeDefined();
    const scriptSrc = csp!.value
      .split(";")
      .map((s) => s.trim())
      .find((s) => s.startsWith("script-src"));
    expect(scriptSrc).toBeDefined();
    expect(scriptSrc).not.toContain("'unsafe-eval'");
  });

  it("permits 'unsafe-eval' in non-prod for dev hot-reloading", () => {
    const headers = buildSecurityHeaders({ isProd: false });
    const csp = headers.find((h) => h.key === "Content-Security-Policy");
    expect(csp).toBeDefined();
    expect(csp!.value).toContain("'unsafe-eval'");
  });

  it("asserts connect-src includes apiOrigin", () => {
    const origin = "https://api.sniper-suite.trading";
    const headers = buildSecurityHeaders({ apiOrigin: origin, isProd: true });
    const csp = headers.find((h) => h.key === "Content-Security-Policy");
    expect(csp).toBeDefined();
    const connectSrc = csp!.value
      .split(";")
      .map((s) => s.trim())
      .find((s) => s.startsWith("connect-src"));
    expect(connectSrc).toBeDefined();
    expect(connectSrc).toContain(origin);
    expect(connectSrc).toContain("'self'");
  });

  it("asserts HSTS only in prod", () => {
    const prodHeaders = buildSecurityHeaders({ isProd: true });
    const prodHsts = prodHeaders.find((h) => h.key === "Strict-Transport-Security");
    expect(prodHsts).toBeDefined();
    expect(prodHsts!.value).toBe("max-age=31536000; includeSubDomains");

    const devHeaders = buildSecurityHeaders({ isProd: false });
    const devHsts = devHeaders.find((h) => h.key === "Strict-Transport-Security");
    expect(devHsts).toBeUndefined();
  });

  it("asserts X-Frame-Options DENY", () => {
    const headers = buildSecurityHeaders({ isProd: true });
    const xfo = headers.find((h) => h.key === "X-Frame-Options");
    expect(xfo).toBeDefined();
    expect(xfo!.value).toBe("DENY");
  });

  it("asserts Permissions-Policy camera, microphone, geolocation, and payment off", () => {
    const headers = buildSecurityHeaders({ isProd: true });
    const pp = headers.find((h) => h.key === "Permissions-Policy");
    expect(pp).toBeDefined();
    expect(pp!.value).toContain("camera=()");
    expect(pp!.value).toContain("microphone=()");
    expect(pp!.value).toContain("geolocation=()");
    expect(pp!.value).toContain("payment=()");
  });

  it("asserts COOP and CORP are set to same-origin", () => {
    const headers = buildSecurityHeaders({ isProd: true });
    const coop = headers.find((h) => h.key === "Cross-Origin-Opener-Policy");
    const corp = headers.find((h) => h.key === "Cross-Origin-Resource-Policy");
    expect(coop?.value).toBe("same-origin");
    expect(corp?.value).toBe("same-origin");
  });
});
