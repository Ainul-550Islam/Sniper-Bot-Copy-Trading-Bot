"use client";

/**
 * Public invitation acceptance page.
 *
 * The invitation token comes from the invitation URL and is submitted only
 * over the same-origin API request. It is never written to localStorage,
 * sessionStorage, cookies, analytics, or the URL after acceptance. A newly
 * accepted invitation returns a normal in-memory session and redirects to
 * the authenticated control plane.
 */

import { type FormEvent, useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import { ApiError, auth } from "@/lib/api";
import { establishSession } from "@/lib/auth";

const MIN_PASSWORD_LENGTH = 12;

function errorMessage(error: unknown): string {
  if (error instanceof ApiError) {
    return error.reason || error.message;
  }
  if (error instanceof Error) {
    return error.message;
  }
  return "The invitation could not be accepted.";
}

export default function AcceptInvitePage() {
  const router = useRouter();
  const [token, setToken] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [loading, setLoading] = useState(false);
  const [accepted, setAccepted] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const queryToken = new URLSearchParams(window.location.search).get("token") ?? "";
    window.history.replaceState({}, document.title, window.location.pathname);
    void Promise.resolve().then(() => setToken(queryToken.trim()));
  }, []);

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    if (!token) {
      setError("The invitation link does not contain a token.");
      return;
    }
    if (password.length < MIN_PASSWORD_LENGTH) {
      setError(`Password must be at least ${MIN_PASSWORD_LENGTH} characters.`);
      return;
    }
    if (password !== confirmPassword) {
      setError("Password confirmation does not match.");
      return;
    }

    setLoading(true);
    try {
      const response = await auth.acceptInvite(token, password, displayName.trim() || undefined);
      establishSession(response);
      setAccepted(true);
      window.setTimeout(() => router.replace("/"), 400);
    } catch (caught: unknown) {
      setError(errorMessage(caught));
    } finally {
      setLoading(false);
    }
  }

  return (
    <main
      style={{
        minHeight: "100vh",
        display: "grid",
        placeItems: "center",
        padding: "2rem 1rem",
        background: "var(--bg, #07111f)",
        color: "var(--txt, #f4f7fb)",
      }}
    >
      <section
        style={{
          width: "min(100%, 30rem)",
          maxWidth: "30rem",
          padding: "2rem",
          border: "1px solid var(--line, #24344a)",
          borderRadius: "16px",
          background: "var(--panel, #0e1a2b)",
          boxShadow: "0 20px 60px rgb(0 0 0 / 24%)",
        }}
      >
        <p style={{ margin: 0, color: "var(--muted, #9fb0c5)", fontSize: "0.8rem", letterSpacing: "0.08em", textTransform: "uppercase" }}>
          Sniper Suite Control Plane
        </p>
        <h1 style={{ margin: "0.5rem 0 0.75rem" }}>Accept invitation</h1>
        <p style={{ margin: "0 0 1.5rem", color: "var(--muted, #9fb0c5)", lineHeight: 1.6 }}>
          Create your account and join the organization associated with this one-time invitation.
        </p>

        {accepted ? (
          <div
            role="status"
            style={{
              padding: "1rem",
              borderRadius: "10px",
              background: "rgb(39 174 96 / 14%)",
              color: "#8be0ad",
              lineHeight: 1.5,
            }}
          >
            Invitation accepted. Your secure session is active; redirecting to the control plane.
          </div>
        ) : (
          <form onSubmit={submit}>
            <label style={{ display: "grid", gap: "0.4rem", marginBottom: "1rem" }}>
              <span>Display name</span>
              <input
                value={displayName}
                onChange={(event) => setDisplayName(event.target.value)}
                autoComplete="name"
                maxLength={160}
                style={inputStyle}
              />
            </label>

            <label style={{ display: "grid", gap: "0.4rem", marginBottom: "1rem" }}>
              <span>Password</span>
              <input
                value={password}
                onChange={(event) => setPassword(event.target.value)}
                type="password"
                autoComplete="new-password"
                minLength={MIN_PASSWORD_LENGTH}
                required
                style={inputStyle}
              />
            </label>

            <label style={{ display: "grid", gap: "0.4rem", marginBottom: "1rem" }}>
              <span>Confirm password</span>
              <input
                value={confirmPassword}
                onChange={(event) => setConfirmPassword(event.target.value)}
                type="password"
                autoComplete="new-password"
                minLength={MIN_PASSWORD_LENGTH}
                required
                style={inputStyle}
              />
            </label>

            {error ? (
              <div
                role="alert"
                style={{
                  marginBottom: "1rem",
                  padding: "0.8rem",
                  borderRadius: "10px",
                  background: "rgb(231 76 60 / 14%)",
                  color: "#ffaaa0",
                  lineHeight: 1.5,
                }}
              >
                {error}
              </div>
            ) : null}

            <button type="submit" disabled={loading || !token} style={buttonStyle}>
              {loading ? "Accepting invitation..." : "Accept invitation"}
            </button>
            {!token ? (
              <p style={{ margin: "0.8rem 0 0", color: "#ffaaa0", fontSize: "0.9rem" }}>
                This page must be opened from a valid invitation link.
              </p>
            ) : null}
          </form>
        )}
      </section>
    </main>
  );
}

const inputStyle = {
  width: "100%",
  boxSizing: "border-box" as const,
  padding: "0.75rem 0.8rem",
  border: "1px solid var(--line, #30435d)",
  borderRadius: "8px",
  background: "var(--panel2, #091523)",
  color: "var(--txt, #f4f7fb)",
  font: "inherit",
};

const buttonStyle = {
  width: "100%",
  padding: "0.8rem 1rem",
  border: 0,
  borderRadius: "8px",
  background: "#4f8cff",
  color: "white",
  font: "inherit",
  fontWeight: 700,
  cursor: "pointer",
};
