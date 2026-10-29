// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { proxy, config } from "../proxy";
import { CSP_NONCE_HEADER } from "../lib/security-headers";

/**
 * The proxy runs in the Edge runtime, where `Request`/`Headers`/`crypto` are the
 * WHATWG globals. jsdom supplies the first two and Node 20 supplies
 * `crypto.getRandomValues`, so the real handler can be exercised directly.
 */
function request(url = "https://console.example.test/operator/modules"): Request {
  return new Request(url, { headers: { accept: "text/html" } });
}

function nonceOf(csp: string): string {
  const scriptSrc = csp
    .split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith("script-src"));
  const match = /'nonce-([A-Za-z0-9+/=_-]+)'/.exec(scriptSrc ?? "");
  expect(match, "no nonce in script-src").not.toBeNull();
  return (match as RegExpExecArray)[1] as string;
}

describe("proxy", () => {
  it("sets the CSP header on the response", () => {
    const response = proxy(request() as never);
    expect(response.headers.get("Content-Security-Policy")).toBeTruthy();
  });

  it("mints a unique nonce per request and puts that exact nonce in the CSP", () => {
    const first = proxy(request() as never);
    const second = proxy(request() as never);

    const firstCsp = first.headers.get("Content-Security-Policy") as string;
    const secondCsp = second.headers.get("Content-Security-Policy") as string;

    const firstNonce = nonceOf(firstCsp);
    const secondNonce = nonceOf(secondCsp);

    expect(firstNonce).not.toBe(secondNonce);
    expect(firstCsp).toContain(`'nonce-${firstNonce}'`);
    expect(firstCsp).not.toContain(`'nonce-${secondNonce}'`);
  });

  it("forwards the same nonce to server components on x-nonce", () => {
    const response = proxy(request() as never);
    const csp = response.headers.get("Content-Security-Policy") as string;

    // NextResponse.next({ request: { headers } }) publishes the rewritten
    // request headers on the response: `x-middleware-override-headers` lists
    // the overridden names and `x-middleware-request-<name>` carries the value
    // the downstream render sees when it calls headers().get("x-nonce").
    const overridden = response.headers.get("x-middleware-override-headers");
    expect(overridden).not.toBeNull();
    expect((overridden as string).split(",")).toContain(CSP_NONCE_HEADER);
    expect(response.headers.get(`x-middleware-request-${CSP_NONCE_HEADER}`)).toBe(
      nonceOf(csp),
    );
  });

  it("never emits an inline-script escape hatch", () => {
    const env = process.env as Record<string, string | undefined>;
    for (const nodeEnv of ["development", "production"]) {
      const previous = env.NODE_ENV;
      env.NODE_ENV = nodeEnv;
      try {
        const csp = proxy(request() as never).headers.get(
          "Content-Security-Policy",
        ) as string;
        const scriptSrc = csp
          .split(";")
          .map((part) => part.trim())
          .find((part) => part.startsWith("script-src")) as string;
        expect(scriptSrc).not.toContain("'unsafe-inline'");
        expect(scriptSrc).toContain("'strict-dynamic'");
      } finally {
        env.NODE_ENV = previous;
      }
    }
  });

  it("excludes static assets, the image optimiser and the favicon", () => {
    const matcher = (config.matcher as string[])[0];
    expect(matcher).toContain("_next/static");
    expect(matcher).toContain("_next/image");
    expect(matcher).toContain("favicon");
    // Sanity: the negative lookahead really does skip those paths.
    const skip = new RegExp(`^${matcher}$`);
    expect(skip.test("/_next/static/chunk.js")).toBe(false);
    expect(skip.test("/_next/image?url=%2Flogo.svg")).toBe(false);
    expect(skip.test("/favicon.ico")).toBe(false);
    expect(skip.test("/operator/modules")).toBe(true);
  });
});
