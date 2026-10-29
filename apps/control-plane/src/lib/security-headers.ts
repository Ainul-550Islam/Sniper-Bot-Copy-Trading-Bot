/**
 * HTTP security headers configuration for the control plane Next.js application.
 *
 * Provides:
 * - Content-Security-Policy (CSP) with restrictive defaults, frame-ancestors 'none',
 *   and no 'unsafe-eval' in production.
 * - X-Frame-Options: DENY (clickjacking protection)
 * - X-Content-Type-Options: nosniff
 * - Strict-Transport-Security (HSTS): 1 year + includeSubDomains in production only
 * - Permissions-Policy: camera, microphone, geolocation, and payment disabled
 * - Cross-Origin-Opener-Policy: same-origin
 * - Cross-Origin-Resource-Policy: same-origin
 */

export interface SecurityHeadersOptions {
  apiOrigin?: string;
  isProd?: boolean;
}

export interface HeaderEntry {
  key: string;
  value: string;
}

/**
 * Builds the security headers array for Next.js responses.
 */
export function buildSecurityHeaders(options: SecurityHeadersOptions = {}): HeaderEntry[] {
  const isProd = Boolean(options.isProd);
  const apiOrigin = options.apiOrigin?.trim();

  // CSP connect-src: 'self' + optional apiOrigin
  const connectSrcList = ["'self'"];
  if (apiOrigin && apiOrigin !== "''" && apiOrigin !== '""') {
    connectSrcList.push(apiOrigin);
  }

  // Script-src: in production strictly disallow 'unsafe-eval'
  const scriptSrcList = ["'self'", "'unsafe-inline'"];
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

  const headers: HeaderEntry[] = [
    { key: "Content-Security-Policy", value: cspDirectives.join("; ") },
    { key: "X-Frame-Options", value: "DENY" },
    { key: "X-Content-Type-Options", value: "nosniff" },
    { key: "Referrer-Policy", value: "strict-origin-when-cross-origin" },
    { key: "Permissions-Policy", value: "camera=(), microphone=(), geolocation=(), payment=()" },
    { key: "Cross-Origin-Opener-Policy", value: "same-origin" },
    { key: "Cross-Origin-Resource-Policy", value: "same-origin" },
  ];

  // HSTS: 1 year (31,536,000 seconds) with includeSubDomains, production only
  if (isProd) {
    headers.push({
      key: "Strict-Transport-Security",
      value: "max-age=31536000; includeSubDomains",
    });
  }

  return headers;
}
