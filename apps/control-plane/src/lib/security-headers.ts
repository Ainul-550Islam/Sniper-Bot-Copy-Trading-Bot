/**
 * HTTP security headers configuration for the control plane Next.js application.
 *
 * Split in two on purpose:
 *
 * - {@link buildStaticSecurityHeaders} emits the headers that are identical for
 *   every response and therefore belong in `next.config.ts` `headers()`
 *   (XFO, nosniff, Referrer-Policy, Permissions-Policy, COOP, CORP, HSTS).
 * - {@link buildCsp} emits the Content-Security-Policy, which is DIFFERENT on
 *   every request because `script-src` carries a freshly minted per-request
 *   nonce. A nonce that is baked into the build config is not a nonce, so the
 *   CSP is set by `src/proxy.ts` instead.
 *
 * Provides:
 * - Content-Security-Policy (CSP) with a per-request nonce + 'strict-dynamic',
 *   frame-ancestors 'none', and no 'unsafe-eval' in production.
 * - X-Frame-Options: DENY (clickjacking protection)
 * - X-Content-Type-Options: nosniff
 * - Strict-Transport-Security (HSTS): 1 year + includeSubDomains in production only
 * - Permissions-Policy: camera, microphone, geolocation, and payment disabled
 * - Cross-Origin-Opener-Policy: same-origin
 * - Cross-Origin-Resource-Policy: same-origin
 */

/**
 * Request header that carries the per-request nonce from `src/proxy.ts` to
 * server components. Single owner, so the proxy, the layout and the tests
 * cannot drift onto different names.
 */
export const CSP_NONCE_HEADER = "x-nonce";

export interface StaticSecurityHeadersOptions {
  isProd?: boolean;
}

export interface CspOptions {
  /**
   * Crypto-random nonce minted once per request by `src/proxy.ts`. When absent
   * the policy degrades to `script-src 'self'` — never to `'unsafe-inline'`.
   */
  nonce?: string;
  apiOrigin?: string;
  isProd?: boolean;
}

export interface HeaderEntry {
  key: string;
  value: string;
}

/** Characters a nonce may contain (base64/hex alphabet, no CSP metacharacters). */
const NONCE_ALPHABET = /^[A-Za-z0-9+/=_-]+$/;

/**
 * A nonce is interpolated into the policy verbatim, so a value containing a
 * quote, semicolon or whitespace could append extra sources to `script-src`.
 * Anything that is not pure nonce alphabet is rejected rather than escaped.
 */
function normaliseNonce(nonce: string | undefined): string | null {
  const trimmed = nonce?.trim();
  if (!trimmed) return null;
  return NONCE_ALPHABET.test(trimmed) ? trimmed : null;
}

function normaliseOrigin(apiOrigin: string | undefined): string | null {
  const trimmed = apiOrigin?.trim();
  // `next.config.ts` passes `""` for same-origin deployments and the literal
  // string may itself arrive quoted; both mean "no extra origin".
  if (!trimmed || trimmed === "''" || trimmed === '""') return null;
  return trimmed;
}

/**
 * Builds the per-request Content-Security-Policy.
 *
 * - `script-src 'self' 'nonce-<n>' 'strict-dynamic'`: scripts must either come
 *   from this origin or carry this request's nonce. `'strict-dynamic'` lets a
 *   nonced loader script create further scripts, which is exactly what Next.js'
 *   client runtime does; it also makes host allowlists inert in the browsers
 *   that support it, so the nonce is the only way in.
 * - `'unsafe-inline'` is deliberately absent from `script-src` in every
 *   environment, and `'unsafe-eval'` is added only outside production (Next's
 *   dev overlay evaluates source strings locally).
 * - `style-src` keeps `'unsafe-inline'`: React renders the inline `style={{…}}`
 *   props used throughout this app as `style` ATTRIBUTES, and CSP has no nonce
 *   channel for attributes — only `'unsafe-inline'` or `'unsafe-hashes'` with a
 *   per-attribute hash list. Removing it breaks the rendered UI, not an attack
 *   path, so it stays.
 * - `connect-src` is `'self'` plus the configured API origin (a split
 *   deployment talks to a different host for `/api/**`).
 */
export function buildCsp(options: CspOptions = {}): string {
  const isProd = Boolean(options.isProd);

  const connectSrcList = ["'self'"];
  const origin = normaliseOrigin(options.apiOrigin);
  if (origin) connectSrcList.push(origin);

  const scriptSrcList = ["'self'"];
  const nonce = normaliseNonce(options.nonce);
  if (nonce) {
    scriptSrcList.push(`'nonce-${nonce}'`, "'strict-dynamic'");
  }
  if (!isProd) {
    scriptSrcList.push("'unsafe-eval'");
  }

  const cspDirectives = [
    "default-src 'self'",
    `script-src ${scriptSrcList.join(" ")}`,
    "style-src 'self' 'unsafe-inline'",
    "img-src 'self' data:",
    `connect-src ${connectSrcList.join(" ")}`,
    "frame-ancestors 'none'",
    "base-uri 'self'",
    "form-action 'self'",
    "object-src 'none'",
  ];

  return cspDirectives.join("; ");
}

/**
 * Builds the response headers that do NOT vary per request.
 *
 * There is deliberately no Content-Security-Policy here: see {@link buildCsp}.
 */
export function buildStaticSecurityHeaders(
  options: StaticSecurityHeadersOptions = {},
): HeaderEntry[] {
  const headers: HeaderEntry[] = [
    { key: "X-Frame-Options", value: "DENY" },
    { key: "X-Content-Type-Options", value: "nosniff" },
    { key: "Referrer-Policy", value: "strict-origin-when-cross-origin" },
    { key: "Permissions-Policy", value: "camera=(), microphone=(), geolocation=(), payment=()" },
    { key: "Cross-Origin-Opener-Policy", value: "same-origin" },
    { key: "Cross-Origin-Resource-Policy", value: "same-origin" },
  ];

  // HSTS: 1 year (31,536,000 seconds) with includeSubDomains, production only.
  if (options.isProd) {
    headers.push({
      key: "Strict-Transport-Security",
      value: "max-age=31536000; includeSubDomains",
    });
  }

  return headers;
}
