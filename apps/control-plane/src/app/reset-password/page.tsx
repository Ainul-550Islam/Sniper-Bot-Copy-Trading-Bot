"use client";

/**
 * Reset-password page (GAP MAP v2, Part 5).
 *
 * Landing target of the emailed reset link. Reads the single-use `token` from
 * the query string and submits the new password to
 * `POST /api/saas/password-reset/confirm`. The backend invalidates the token
 * on success and signs out every existing session; every failure (expired,
 * revoked, used, unknown) returns the same opaque refusal, which this page
 * surfaces without guessing the cause.
 *
 * Wrapped in Suspense because `useSearchParams()` requires a boundary during
 * prerendering.
 */
import { Suspense, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import Link from "next/link";
import { ApiError, request } from "@/lib/api";

interface ConfirmResponse {
  status: string;
  message: string;
}

export default function ResetPasswordPage() {
  return (
    <Suspense fallback={null}>
      <ResetPasswordBody />
    </Suspense>
  );
}

function ResetPasswordBody() {
  const router = useRouter();
  const token = useSearchParams().get("token");
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<ConfirmResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setError(null);
    if (!token) return;
    if (password !== confirmPassword) {
      setError("The two passwords do not match.");
      return;
    }
    setBusy(true);
    try {
      const res = await request<ConfirmResponse>("/api/saas/password-reset/confirm", {
        method: "POST",
        body: { token, new_password: password },
        anonymous: true,
      });
      setDone(res);
    } catch (err) {
      setError(err instanceof ApiError ? err.reason : "Could not reset the password.");
    } finally {
      setBusy(false);
    }
  }

  return (
    <main className="landing" style={{ minHeight: "100vh", display: "grid", placeItems: "center", padding: "1rem" }}>
      <div className="card form" style={{ maxWidth: "420px", width: "100%" }}>
        <h1 style={{ marginTop: 0, fontSize: "1.2rem" }}>Choose a new password</h1>

        {!token ? (
          <>
            <p className="error" role="alert">
              This reset link is missing its token. Request a new one.
            </p>
            <Link href="/forgot-password" className="primary" style={{ display: "inline-block", textDecoration: "none" }}>
              Request a new link
            </Link>
          </>
        ) : done ? (
          <>
            <div className="card" style={{ background: "var(--ok-glow)", borderColor: "rgba(16, 185, 129, 0.4)" }}>
              <p style={{ margin: 0 }}>{done.message}</p>
            </div>
            <button className="primary" type="button" onClick={() => router.push("/")}>
              Go to sign in
            </button>
          </>
        ) : (
          <form onSubmit={submit} aria-label="Set a new password">
            <label>
              New password
              <input
                type="password"
                required
                minLength={12}
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                autoComplete="new-password"
              />
              <small className="muted">At least 12 characters.</small>
            </label>
            <label>
              Repeat new password
              <input
                type="password"
                required
                minLength={12}
                value={confirmPassword}
                onChange={(e) => setConfirmPassword(e.target.value)}
                autoComplete="new-password"
              />
            </label>
            {error && (
              <p className="error" role="alert">
                {error}
              </p>
            )}
            <button className="primary" disabled={busy || password.length < 12} type="submit">
              {busy ? "Saving…" : "Set new password"}
            </button>
            <p className="muted small" style={{ marginTop: "0.75rem" }}>
              <Link href="/forgot-password">Link expired or not working? Request a new one.</Link>
            </p>
          </form>
        )}
      </div>
    </main>
  );
}
