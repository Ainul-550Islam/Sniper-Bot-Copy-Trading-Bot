"use client";

import { Suspense, useEffect, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";

import { AppShell } from "@/components/AppShell";
import ConsentCheckbox from "@/components/legal/ConsentCheckbox";
import { ApiError } from "@/lib/api";
import { hasLiveSession, login, register, sessionStore } from "@/lib/auth";
import type { ConsentRecord } from "@/lib/consent";

/**
 * The auth-aware landing page (TASK 7B file 05).
 *
 * Signed out → sign-in / sign-up. Signed in → the {@link AppShell}. The
 * page talks to the backend exclusively through `lib/auth` (which talks to
 * `lib/api`); no component here crafts fetch calls.
 *
 * Wrapped in Suspense because the signed-out form reads `useSearchParams()`
 * (the invite deep-link), which Next.js requires to be behind a boundary
 * during prerendering.
 */
export default function Page() {
  return (
    <Suspense fallback={null}>
      <PageBody />
    </Suspense>
  );
}

function PageBody() {
  const [, forceRender] = useState(0);

  // Keep the (externally stored) session state in render.
  useEffect(() => sessionStore.subscribe(() => forceRender((n) => n + 1)), []);

  const signedIn = hasLiveSession();
  return signedIn ? <AppShell /> : <AuthLanding />;
}

function AuthLanding() {
  const router = useRouter();
  // Invite deep-links (?organization=slug) pre-fill the MFA-protected login.
  const invitedOrganization = useSearchParams().get("organization");
  const [mode, setMode] = useState<"login" | "register">("login");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [organization, setOrganization] = useState(invitedOrganization ?? "");
  const [mfaCode, setMfaCode] = useState("");
  const [mfaRequired, setMfaRequired] = useState(invitedOrganization !== null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Sign-up consent (version + timestamp), captured by ConsentCheckbox.
  // `null` = not accepted; registration is blocked until it is.
  const [consent, setConsent] = useState<ConsentRecord | null>(null);

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      if (mode === "login") {
        const enrollmentOnly = await login(
          email,
          password,
          organization.trim() || undefined,
          mfaCode.trim() || undefined,
        );
        if (enrollmentOnly) {
          router.replace("/mfa-enrollment");
          return;
        }
        setMfaRequired(false);
      } else {
        if (!consent) {
          setError("Please read and accept the Terms, Privacy Policy and Risk Disclosure to create an account.");
          return;
        }
        await register(email, password, displayName || (email.split("@")[0] ?? ""), consent);
      }
    } catch (err) {
      if (
        err instanceof ApiError &&
        ["organization_required_for_mfa", "mfa_challenge_required", "invalid_totp_code"].includes(err.kind)
      ) {
        setMfaRequired(true);
        setError(
          err.kind === "organization_required_for_mfa"
            ? "This account belongs to an MFA-protected organization. Enter its slug and a current authenticator code."
            : err.kind === "mfa_challenge_required"
              ? "Enter the current six-digit code from your authenticator app."
              : "That authenticator code is invalid or already used. Wait for a fresh code and try again.",
        );
      } else {
        setError(err instanceof Error ? err.message : "sign-in failed");
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <main id="main" className="landing">
      <header className="landing__brand">
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img src="/logo.svg" alt="" width={44} height={44} />
        <div>
          <h1>Sniper Suite — Control Plane</h1>
          <p className="muted">
            Tenants, wallets, billing and audit. Trading truth lives in the
            operator engines; this console manages access to it.
          </p>
        </div>
      </header>

      <form className="card form" onSubmit={submit} aria-label="Sign in">
        <div className="tabs" role="tablist">
          <button
            type="button"
            role="tab"
            aria-selected={mode === "login"}
            onClick={() => {
              setMode("login");
              setConsent(null);
            }}
          >
            Sign in
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={mode === "register"}
            onClick={() => setMode("register")}
          >
            Create account
          </button>
        </div>

        {mode === "register" && (
          <label>
            Display name
            <input
              value={displayName}
              onChange={(e) => setDisplayName(e.target.value)}
              autoComplete="nickname"
              maxLength={120}
            />
          </label>
        )}
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
        <label>
          Password
          <input
            type="password"
            required
            minLength={mode === "register" ? 12 : 1}
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete={mode === "register" ? "new-password" : "current-password"}
          />
          {mode === "register" && (
            <small className="muted">At least 12 characters.</small>
          )}
        </label>

        {mode === "register" && (
          <ConsentCheckbox consent={consent} onChange={setConsent} disabled={busy} />
        )}

        {mode === "login" && mfaRequired && (
          <>
            <label>
              Organization slug
              <input
                value={organization}
                onChange={(event) => setOrganization(event.target.value)}
                autoComplete="organization"
                required
                maxLength={64}
                placeholder="acme-capital"
              />
            </label>
            <label>
              Authenticator code
              <input
                value={mfaCode}
                onChange={(event) => setMfaCode(event.target.value.replace(/\D/g, "").slice(0, 6))}
                inputMode="numeric"
                autoComplete="one-time-code"
                pattern="[0-9]{6}"
                maxLength={6}
                placeholder="123456"
              />
              <small className="muted">Leave blank if you are enrolling an authenticator for the first time.</small>
            </label>
          </>
        )}

        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
        <button
          className="primary"
          disabled={busy || (mode === "register" && consent === null)}
          type="submit"
        >
          {busy ? "Working…" : mode === "login" ? "Sign in" : "Create account"}
        </button>
        {mode === "register" && consent === null && (
          <p className="muted small">Accept the legal documents above to enable account creation.</p>
        )}
        {mode === "login" && (
          <p className="muted small">
            <a href="/forgot-password">Forgot your password?</a>{" "}
            ·{" "}
            <a href="/verify-email">Re-send email verification</a>
          </p>
        )}
        <p className="muted small">
          The session token is kept in this tab&apos;s memory only. Closing or
          reloading the page signs you out.
        </p>
      </form>
    </main>
  );
}
