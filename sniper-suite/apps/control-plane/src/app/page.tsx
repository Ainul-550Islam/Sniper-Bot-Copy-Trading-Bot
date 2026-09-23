"use client";

import { useEffect, useState } from "react";

import { AppShell } from "@/components/AppShell";
import { hasLiveSession, login, register, sessionStore } from "@/lib/auth";

/**
 * The auth-aware landing page (TASK 7B file 05).
 *
 * Signed out → sign-in / sign-up. Signed in → the {@link AppShell}. The
 * page talks to the backend exclusively through `lib/auth` (which talks to
 * `lib/api`); no component here crafts fetch calls.
 */
export default function Page() {
  const state = sessionStore.getSnapshot();
  const [, forceRender] = useState(0);

  // Keep the (externally stored) session state in render.
  useEffect(() => sessionStore.subscribe(() => forceRender((n) => n + 1)), []);

  const signedIn = hasLiveSession();
  return signedIn ? <AppShell /> : <AuthLanding />;
}

function AuthLanding() {
  const [mode, setMode] = useState<"login" | "register">("login");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      if (mode === "login") {
        await login(email, password);
      } else {
        await register(email, password, displayName || (email.split("@")[0] ?? ""));
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : "sign-in failed");
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
            onClick={() => setMode("login")}
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

        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
        <button className="primary" disabled={busy} type="submit">
          {busy ? "Working…" : mode === "login" ? "Sign in" : "Create account"}
        </button>
        <p className="muted small">
          The session token is kept in this tab's memory only. Closing or
          reloading the page signs you out.
        </p>
      </form>
    </main>
  );
}
