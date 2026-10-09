"use client";

import { useEffect, useState } from "react";
import { useRouter } from "next/navigation";
import Link from "next/link";
import { setupTotp, verifyTotp } from "@/lib/api/security-api";
import { needsMfaEnrollment, refresh } from "@/lib/auth";
import { ApiError } from "@/lib/api";
import TotpQrCode from "@/components/settings/TotpQrCode";

function message(error: unknown): string {
  if (error instanceof ApiError) return error.reason || error.message;
  return error instanceof Error ? error.message : "Authenticator enrollment failed.";
}

export default function MfaEnrollmentPage() {
  const router = useRouter();
  const [device, setDevice] = useState<{ id: string; secret: string; uri: string } | null>(null);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The enrollment-only session exists only right after an MFA-gated login;
  // derive the expired state once from the in-memory session instead of
  // setting state from an effect.
  const [expired] = useState(() => !needsMfaEnrollment());

  useEffect(() => {
    if (!needsMfaEnrollment()) return;
    void createSetup();
    // The initial session is established by the login flow before navigation.
  }, []);

  async function createSetup() {
    setBusy(true);
    setError(null);
    try {
      const setup = await setupTotp();
      setDevice({ id: setup.device_id, secret: setup.secret, uri: setup.otpauth_url });
    } catch (caught: unknown) {
      setError(message(caught));
    } finally {
      setBusy(false);
    }
  }

  async function verify() {
    if (!device || !/^\d{6}$/.test(code)) {
      setError("Enter the current six-digit code from your authenticator app.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const result = await verifyTotp(device.id, code);
      if (!result.session_promoted) {
        throw new Error("TOTP was verified, but the restricted session was not activated.");
      }
      if (!(await refresh())) {
        throw new Error("TOTP was verified, but the session expired. Sign in again with your authenticator code.");
      }
      router.replace("/");
    } catch (caught: unknown) {
      setError(message(caught));
    } finally {
      setBusy(false);
    }
  }

  return (
    <main style={pageStyle}>
      <section style={cardStyle}>
        <p style={{ margin: 0, color: "#9fb0c5", fontSize: "0.8rem", letterSpacing: "0.08em", textTransform: "uppercase" }}>
          Sniper Suite Control Plane
        </p>
        <h1 style={{ margin: "0.5rem 0 0.75rem" }}>Set up your authenticator</h1>
        {expired ? (
          <>
            <p style={{ color: "#9fb0c5", lineHeight: 1.6 }}>
              This enrollment session is missing or expired. Sign in again to continue.
            </p>
            <Link href="/" style={{ color: "#8eb9ff" }}>Return to sign in</Link>
          </>
        ) : (
          <>
            <p style={{ color: "#9fb0c5", lineHeight: 1.6 }}>
              Your organization requires multi-factor authentication. The temporary session permits only authenticator setup and verification.
            </p>
            {!device ? (
              <button type="button" disabled={busy} onClick={() => void createSetup()} style={buttonStyle}>
                {busy ? "Preparing authenticator…" : "Retry authenticator setup"}
              </button>
            ) : (
              <div style={{ display: "grid", gap: "0.8rem" }}>
                <p style={{ margin: 0 }}>Scan the code, or add the secret manually, in your authenticator app:</p>
                <div style={{ justifySelf: "start" }}>
                  <TotpQrCode value={device.uri} size={160} label="Scan to enroll in your authenticator app" />
                </div>
                <code style={{ overflowWrap: "anywhere", userSelect: "all", padding: "0.8rem", background: "#091523", borderRadius: "8px" }}>
                  {device.secret}
                </code>
                <label style={{ display: "grid", gap: "0.4rem" }}>
                  <span>Current six-digit code</span>
                  <input
                    value={code}
                    onChange={(event) => setCode(event.target.value.replace(/\D/g, "").slice(0, 6))}
                    inputMode="numeric"
                    autoComplete="one-time-code"
                    maxLength={6}
                    pattern="[0-9]{6}"
                    style={inputStyle}
                  />
                </label>
                <button type="button" disabled={busy || !/^\d{6}$/.test(code)} onClick={() => void verify()} style={buttonStyle}>
                  {busy ? "Verifying…" : "Verify and activate session"}
                </button>
              </div>
            )}
            {error ? <p role="alert" style={{ color: "#ffaaa0", lineHeight: 1.5 }}>{error}</p> : null}
          </>
        )}
      </section>
    </main>
  );
}

const pageStyle = {
  minHeight: "100vh",
  display: "grid",
  placeItems: "center",
  padding: "2rem 1rem",
  background: "var(--bg, #07111f)",
  color: "var(--txt, #f4f7fb)",
};

const cardStyle = {
  width: "min(100%, 30rem)",
  boxSizing: "border-box" as const,
  padding: "2rem",
  border: "1px solid var(--line, #24344a)",
  borderRadius: "16px",
  background: "var(--panel, #0e1a2b)",
  boxShadow: "0 20px 60px rgb(0 0 0 / 24%)",
};

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
