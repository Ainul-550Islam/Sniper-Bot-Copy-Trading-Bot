"use client";

import { useState } from "react";
import { enforceMfa, rotateTokens, updateIpAllowlist } from "@/lib/api/security-api";

interface SecurityFormProps {
  mfaEnforced: boolean;
  ipAllowlist: string[];
  onRefresh: () => void;
}

export default function SecurityForm({ mfaEnforced, ipAllowlist, onRefresh }: SecurityFormProps) {
  const [mfa, setMfa] = useState(mfaEnforced);
  const [cidrs, setCidrs] = useState(ipAllowlist.join("\n"));
  const [loading, setLoading] = useState(false);
  const [msg, setMsg] = useState<{ text: string; ok: boolean } | null>(null);

  const handleMfaToggle = async () => {
    setLoading(true);
    setMsg(null);
    try {
      const next = !mfa;
      await enforceMfa(next);
      setMfa(next);
      setMsg({ text: `MFA enforcement policy updated to ${next ? "ENFORCED" : "OPTIONAL"}`, ok: true });
      onRefresh();
    } catch (err: unknown) {
      setMsg({ text: err instanceof Error ? err.message : "Failed to toggle MFA", ok: false });
    } finally {
      setLoading(false);
    }
  };

  const handleSaveIps = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setMsg(null);
    try {
      const parsed = cidrs
        .split("\n")
        .map((s) => s.trim())
        .filter(Boolean);
      await updateIpAllowlist(parsed);
      setMsg({ text: `IP Allowlist updated with ${parsed.length} rules`, ok: true });
      onRefresh();
    } catch (err: unknown) {
      setMsg({ text: err instanceof Error ? err.message : "Failed to update IP allowlist", ok: false });
    } finally {
      setLoading(false);
    }
  };

  const handleRotateTokens = async () => {
    if (!confirm("This will invalidate all existing API tokens and active sessions. Proceed?")) return;
    setLoading(true);
    setMsg(null);
    try {
      const res = await rotateTokens();
      setMsg({ text: res.message, ok: true });
      onRefresh();
    } catch (err: unknown) {
      setMsg({ text: err instanceof Error ? err.message : "Failed to rotate tokens", ok: false });
    } finally {
      setLoading(false);
    }
  };

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: "1.5rem" }}>
      {msg && (
        <div
          style={{
            color: msg.ok ? "var(--ok)" : "var(--bad)",
            background: msg.ok ? "var(--ok-glow)" : "var(--bad-glow)",
            padding: "0.75rem",
            borderRadius: "6px",
          }}
        >
          {msg.text}
        </div>
      )}

      <div className="card">
        <h3 style={{ marginTop: 0 }}>Multi-Factor Authentication (MFA)</h3>
        <p style={{ color: "var(--muted)", fontSize: "0.85rem" }}>
          Require all tenant members to configure hardware or TOTP multi-factor authentication before accessing
          trading execution surfaces.
        </p>
        <button
          onClick={handleMfaToggle}
          disabled={loading}
          className={`btn ${mfa ? "btn-secondary" : "btn-primary"}`}
        >
          {mfa ? "Disable Mandatory MFA" : "Enforce MFA Org-Wide"}
        </button>
      </div>

      <div className="card">
        <h3 style={{ marginTop: 0 }}>IP Allowlisting & CIDR Filtering</h3>
        <p style={{ color: "var(--muted)", fontSize: "0.85rem" }}>
          Restrict control-plane and trading data-plane API requests to explicit CIDR blocks (one per line).
        </p>
        <form onSubmit={handleSaveIps}>
          <textarea
            rows={4}
            value={cidrs}
            onChange={(e) => setCidrs(e.target.value)}
            placeholder="192.168.1.0/24&#10;10.0.0.0/8"
            className="input"
            style={{ width: "100%", fontFamily: "var(--mono)", fontSize: "0.85rem", marginBottom: "0.75rem" }}
          />
          <button type="submit" disabled={loading} className="btn btn-primary">
            Save IP Rules
          </button>
        </form>
      </div>

      <div className="card" style={{ borderColor: "rgba(239, 68, 68, 0.3)" }}>
        <h3 style={{ marginTop: 0, color: "var(--bad)" }}>Emergency Credential Rotation</h3>
        <p style={{ color: "var(--muted)", fontSize: "0.85rem" }}>
          Immediately revoke all existing tenant API keys, trading plane access tokens, and operator session tokens.
        </p>
        <button onClick={handleRotateTokens} disabled={loading} className="btn btn-secondary" style={{ color: "var(--bad)" }}>
          Rotate All Session Credentials
        </button>
      </div>
    </div>
  );
}
