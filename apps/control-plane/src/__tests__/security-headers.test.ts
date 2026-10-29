import { describe, expect, it } from "vitest";
import {
  buildCsp,
  buildStaticSecurityHeaders,
  CSP_NONCE_HEADER,
} from "../lib/security-headers";

function directive(csp: string, name: string): string {
  const found = csp
    .split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith(name));
  expect(found, `CSP is missing a ${name} directive`).toBeDefined();
  return found as string;
}

describe("security-headers", () => {
  it("buildStaticSecurityHeaders emits the static headers and no CSP", () => {
    const headers = buildStaticSecurityHeaders({ isProd: true });
    const keys = headers.map((h) => h.key);

    expect(keys).toContain("X-Frame-Options");
    expect(keys).toContain("X-Content-Type-Options");
    expect(keys).toContain("Referrer-Policy");
    expect(keys).toContain("Permissions-Policy");
    expect(keys).toContain("Cross-Origin-Opener-Policy");
    expect(keys).toContain("Cross-Origin-Resource-Policy");
    expect(keys).toContain("Strict-Transport-Security");
    // The CSP is per-request (it carries a nonce) and is owned by src/proxy.ts.
    expect(keys).not.toContain("Content-Security-Policy");

    const xfo = headers.find((h) => h.key === "X-Frame-Options");
    expect(xfo?.value).toBe("DENY");
    const coop = headers.find((h) => h.key === "Cross-Origin-Opener-Policy");
    const corp = headers.find((h) => h.key === "Cross-Origin-Resource-Policy");
    expect(coop?.value).toBe("same-origin");
    expect(corp?.value).toBe("same-origin");
    const pp = headers.find((h) => h.key === "Permissions-Policy");
    expect(pp?.value).toContain("camera=()");
    expect(pp?.value).toContain("microphone=()");
    expect(pp?.value).toContain("geolocation=()");
    expect(pp?.value).toContain("payment=()");
  });

  it("omits HSTS outside production", () => {
    const dev = buildStaticSecurityHeaders({ isProd: false });
    expect(dev.find((h) => h.key === "Strict-Transport-Security")).toBeUndefined();
    const prod = buildStaticSecurityHeaders({ isProd: true });
    expect(prod.find((h) => h.key === "Strict-Transport-Security")?.value).toBe(
      "max-age=31536000; includeSubDomains",
    );
  });

  it("names the nonce header the proxy, the layout and the tests share", () => {
    expect(CSP_NONCE_HEADER).toBe("x-nonce");
  });

  it("buildCsp embeds the nonce and 'strict-dynamic' in script-src", () => {
    const csp = buildCsp({ nonce: "abc123nonce", isProd: true });
    const scriptSrc = directive(csp, "script-src");
    expect(scriptSrc).toContain("'nonce-abc123nonce'");
    expect(scriptSrc).toContain("'strict-dynamic'");
    expect(scriptSrc).toContain("'self'");
  });

  it("production script-src has neither 'unsafe-inline' nor 'unsafe-eval'", () => {
    const csp = buildCsp({ nonce: "abc123nonce", isProd: true });
    const scriptSrc = directive(csp, "script-src");
    expect(scriptSrc).not.toContain("'unsafe-inline'");
    expect(scriptSrc).not.toContain("'unsafe-eval'");
    expect(directive(csp, "default-src")).toBe("default-src 'self'");
  });

  it("permits 'unsafe-eval' in non-prod for the dev overlay", () => {
    const csp = buildCsp({ nonce: "abc123nonce", isProd: false });
    expect(directive(csp, "script-src")).toContain("'unsafe-eval'");
    // Even in dev, inline scripts are never allowed.
    expect(directive(csp, "script-src")).not.toContain("'unsafe-inline'");
  });

  it("degrades to 'self' when no nonce is supplied — never to 'unsafe-inline'", () => {
    for (const nonce of [undefined, "", "   "]) {
      const csp = buildCsp({ nonce, isProd: true });
      const scriptSrc = directive(csp, "script-src");
      expect(scriptSrc).toBe("script-src 'self'");
      expect(scriptSrc).not.toContain("'unsafe-inline'");
    }
  });

  it("rejects a nonce that could smuggle extra CSP sources", () => {
    // A nonce is interpolated verbatim; a quote or semicolon would let the
    // value append sources of its own. Such a value is dropped, not escaped.
    const csp = buildCsp({ nonce: "abc' 'unsafe-inline' ;", isProd: true });
    expect(directive(csp, "script-src")).toBe("script-src 'self'");
    expect(csp).not.toContain("'nonce-");
    // style-src legitimately keeps 'unsafe-inline' (React emits inline style
    // attributes); script-src must never gain it, even indirectly.
    expect(directive(csp, "script-src")).not.toContain("'unsafe-inline'");
  });

  it("asserts frame-ancestors 'none' and the baseline directives", () => {
    const csp = buildCsp({ nonce: "abc123nonce", isProd: true });
    expect(directive(csp, "frame-ancestors")).toBe("frame-ancestors 'none'");
    expect(directive(csp, "object-src")).toBe("object-src 'none'");
    expect(directive(csp, "base-uri")).toBe("base-uri 'self'");
    expect(directive(csp, "form-action")).toBe("form-action 'self'");
    expect(directive(csp, "default-src")).toBe("default-src 'self'");
  });

  it("asserts connect-src is 'self' plus the api origin", () => {
    const origin = "https://api.sniper-suite.trading";
    const csp = buildCsp({ nonce: "abc123nonce", apiOrigin: origin, isProd: true });
    const connectSrc = directive(csp, "connect-src");
    expect(connectSrc).toContain("'self'");
    expect(connectSrc).toContain(origin);

    // Same-origin deployments pass "" (or a quoted empty string) and must not
    // produce an empty source expression.
    const same = directive(buildCsp({ nonce: "n1", apiOrigin: "", isProd: true }), "connect-src");
    expect(same).toBe("connect-src 'self'");
    const quoted = directive(buildCsp({ nonce: "n1", apiOrigin: '""', isProd: true }), "connect-src");
    expect(quoted).toBe("connect-src 'self'");
  });

  it("mints a different policy per nonce", () => {
    const a = buildCsp({ nonce: "aaaa", isProd: true });
    const b = buildCsp({ nonce: "bbbb", isProd: true });
    expect(a).not.toBe(b);
    expect(a).toContain("'nonce-aaaa'");
    expect(b).toContain("'nonce-bbbb'");
  });
});
