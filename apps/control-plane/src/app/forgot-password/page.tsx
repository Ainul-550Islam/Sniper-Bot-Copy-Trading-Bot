"use client";

/**
 * Forgot-password page (GAP MAP v2, Part 5).
 *
 * Requests a password-reset email via `POST /api/saas/password-reset/request`.
 * The backend answers with the SAME message whether or not the address exists
 * (no account enumeration), so this page never hints which emails are
 * registered. It shows the backend's accepted message and the reset-link TTL.
 */
import { useState } from "react";
import Link from "next/link";
import { ApiError, request } from "@/lib/api";

interface RequestResponse {
  status: string;
  message: string;
  expires_minutes: number;
}

export default function ForgotPasswordPage() {
  const [email, setEmail] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<RequestResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    setResult(null);
    try {
      const res = await request<RequestResponse>("/api/saas/password-reset/request", {
        method: "POST",
        body: { email: email.trim() },
        anonymous: true,
      });
      setResult(res);
    } catch (err) {
      setError(err instanceof ApiError ? err.reason : "Could not send a reset link. Try again.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="landing" style={{ minHeight: "100vh", display: "grid", placeItems: "center", padding: "1rem" }}>
      <form className="card form" onSubmit={submit} aria-label="Reset your password" style={{ maxWidth: "420px", width: "100%" }}>
        <h1 style={{ marginTop: 0, fontSize: "1.2rem" }}>Reset your password</h1>
        <p className="muted small">
          Enter the email you sign in with. If an account exists for it, we
          will send a reset link.
        </p>

        {result ? (
          <div className="card" style={{ background: "var(--ok-glow)", borderColor: "rgba(16, 185, 129, 0.4)" }}>
            <p style={{ margin: 0 }}>{result.message}</p>
            <p className="muted small" style={{ margin: "0.5rem 0 0" }}>
              The link is valid for {result.expires_minutes} minutes.
            </p>
          </div>
        ) : (
          <>
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
            {error && (
              <p className="error" role="alert">
                {error}
              </p>
            )}
            <button className="primary" disabled={busy || !email.trim()} type="submit">
              {busy ? "Sending…" : "Send reset link"}
            </button>
          </>
        )}

        <p className="muted small" style={{ marginTop: "1rem" }}>
          <Link href="/">Back to sign in</Link>
        </p>
      </form>
    </main>
  );
}
