/**
 * Per-request proxy (Next.js 16 renamed the `middleware` convention to
 * `proxy`; the handler export is `proxy`, see `next/dist/build/templates/
 * middleware.js`).
 *
 * One job: mint a crypto-random CSP nonce for this request, hand it to server
 * components on the request headers, and set the Content-Security-Policy on the
 * response.
 *
 * Why the CSP lives here and not in `next.config.ts` `headers()`: a header
 * produced by `headers()` is a build-time constant, so any nonce in it would be
 * the same for every visitor for the lifetime of the build — the one thing a
 * nonce must never be.
 *
 * Next.js itself re-reads the nonce back off this response's CSP header
 * (`getScriptNonceFromHeader`) and stamps it on the framework scripts it emits,
 * so no `'unsafe-inline'` is needed anywhere.
 */

import { NextResponse, type NextRequest } from "next/server";
import { buildCsp, CSP_NONCE_HEADER } from "@/lib/security-headers";

/** 128 bits of entropy, hex-encoded: unguessable and CSP-alphabet safe. */
const NONCE_BYTES = 16;

function mintNonce(): string {
  const bytes = new Uint8Array(NONCE_BYTES);
  crypto.getRandomValues(bytes);
  let nonce = "";
  for (const byte of bytes) {
    nonce += byte.toString(16).padStart(2, "0");
  }
  return nonce;
}

export function proxy(request: NextRequest): NextResponse {
  const nonce = mintNonce();

  const requestHeaders = new Headers(request.headers);
  requestHeaders.set(CSP_NONCE_HEADER, nonce);

  const response = NextResponse.next({ request: { headers: requestHeaders } });
  response.headers.set(
    "Content-Security-Policy",
    buildCsp({
      nonce,
      apiOrigin: process.env.NEXT_PUBLIC_API_ORIGIN ?? "",
      isProd: process.env.NODE_ENV === "production",
    }),
  );
  return response;
}

export const config = {
  // Static chunks and the image optimiser are immutable, cache-farmed assets;
  // minting a nonce for them would only defeat the CDN cache. The favicon is
  // never a document.
  matcher: ["/((?!_next/static|_next/image|favicon.ico).*)"],
};
