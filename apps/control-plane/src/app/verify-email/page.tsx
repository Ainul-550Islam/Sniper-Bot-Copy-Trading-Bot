"use client";

/**
 * Email-verification landing page (GAP MAP v2, Part 5).
 *
 * Target of the emailed verification link. When a `token` is present it is
 * confirmed against `POST /api/saas/email-verification/confirm`; without a
 * token the page offers to send a fresh verification email via
 * `POST /api/saas/email-verification/request`. The backend gives an identical
 * acceptance for unknown addresses and an identical opaque refusal for every
 * failed confirm, so this page never reveals which emails are registered.
 *
 * Wrapped in Suspense because `useSearchParams()` requires a boundary during
 * prerendering.
 */
import { Suspense, useCallback, useEffect, useState } from "react";
import { useSearchParams } from "next/navigation";
import Link from "next/link";
import { ApiError, request } from "@/lib/api";

interface ConfirmResponse {
  status: string;
  message: string;
}

interface RequestResponse {
  status: string;
  message: string;
}

export default function VerifyEmailPage() {
  return (
    <Suspense fallback={null}>
      <VerifyEmailBody />
    </Suspense>
  );
}

function VerifyEmailBody() {
  const token = useSearchParams().get("token");
  const [email, setEmail] = useState("");
  const [busy, setBusy] = useState(false);
  const [verified, setVerified] = useState<ConfirmResponse | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [resent, setResent] = useState<RequestResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Auto-confirm once when the link carries a token.
  useEffect(() => {
    if (!token) return;
    let cancelled = false;
    (async () => {
      setBusy(true);
      setFailed(null);
      try {
        const res = await request<ConfirmResponse>("/api/saas/email-verification/confirm", {
          method: "POST",
          body: { token },
          anonymous: true,
        });
        if (!cancelled) setVerified(res);
      } catch (err) {
        if (!cancelled) {
          setFailed(err instanceof ApiError ? err.reason : "Could not verify this link.");
        }
      } finally {
        if (!cancelled) setBusy(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [token]);

  const resend = useCallback(
    async (event: React.FormEvent) => {
      event.preventDefault();
      setBusy(true);
      setError(null);
      setResent(null);
      try {
        const res = await request<RequestResponse>("/api/saas/email-verification/request", {
          method: "POST",
          body: { email: email.trim() },
          anonymous: true,
        });
        setResent(res);
      } catch (err) {
        setError(err instanceof ApiError ? err.reason : "Could not send a verification email.");
      } finally {
        setBusy(false);
      }
    },
    [email],
  );

  return (
    <main className="landing" style={{ minHeight: "100vh", display: "grid", placeItems: "center", padding: "1rem" }}>
      <div className="card form" style={{ maxWidth: "440px", width: "100%" }}>
        <h1 style={{ marginTop: 0, fontSize: "1.2rem" }}>Verify your email</h1>

        {token !== null ? (
          busy ? (
            <p className="muted">Checking your verification link…</p>
          ) : verified ? (
            <>
              <div className="card" style={{ background: "var(--ok-glow)", borderColor: "rgba(16, 185, 129, 0.4)" }}>
                <p style={{ margin: 0 }}>{verified.message}</p>
              </div>
              <Link href="/" className="primary" style={{ display: "inline-block", textDecoration: "none", marginTop: "0.75rem" }}>
                Continue to sign in
              </Link>
            </>
          ) : (
            <>
              <p className="error" role="alert">
                {failed ?? "This verification link is no longer valid."}
              </p>
              <p className="muted small">You can request a fresh verification email below.</p>
            </>
          )
        ) : (
          <p className="muted small">Enter your email and we will send you a verification link.</p>
        )}

        {(token === null || (!busy && !verified)) && (
          <form onSubmit={resend} aria-label="Send verification email" style={{ marginTop: "0.5rem" }}>
            <label>
              Email
              <input
                type="email"
                required
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                autoComplete="email"
              />
            </label>
            {error && token === null && (
              <p className="error" role="alert">
                {error}
              </p>
            )}
            {resent && (
              <div className="card" style={{ background: "var(--ok-glow)", borderColor: "rgba(16, 185, 129, 0.4)" }}>
                <p style={{ margin: 0 }}>{resent.message}</p>
              </div>
            )}
            <button className="primary" disabled={busy || !email.trim()} type="submit">
              {busy ? "Sending…" : "Send verification email"}
            </button>
          </form>
        )}

        <p className="muted small" style={{ marginTop: "1rem" }}>
          <Link href="/">Back to sign in</Link>
        </p>
      </div>
    </main>
  );
}
