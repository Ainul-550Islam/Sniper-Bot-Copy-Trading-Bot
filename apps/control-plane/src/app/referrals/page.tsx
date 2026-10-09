"use client";

/**
 * Referrals page (GAP-MAP v2, P2).
 *
 * Shows this organization's referral codes, who referred this organization
 * (and which organizations it referred), and lets owners mint / disable
 * codes or redeem a code they received. Payout policy is intentionally
 * described, not invented: attribution is the durable fact (migration
 * 0049); payout terms are set by the platform and this page never
 * synthesizes amounts.
 */

import { useCallback, useEffect, useState } from "react";
import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { request } from "@/lib/api";
import { branding } from "@/config/branding";

interface ReferralCode {
  code: string;
  status: string;
  created_at: string;
  redemptions: number;
}

interface InboundAttribution {
  referred_by: string;
  referrer_slug: string;
  attributed_at: string;
}

interface OutboundAttribution {
  organization: string;
  slug: string;
  code: string;
  attributed_at: string;
}

interface CodesResponse {
  codes: ReferralCode[];
}

interface AttributionResponse {
  referred_by: InboundAttribution | null;
  referred_organizations: OutboundAttribution[];
}

const cardStyle: React.CSSProperties = {
  padding: "1.25rem",
  borderRadius: "0.5rem",
  border: "1px solid var(--border, #e2e8f0)",
  marginBottom: "1rem",
};

const mutedStyle: React.CSSProperties = {
  color: "var(--muted, #64748b)",
  fontSize: "0.9rem",
};

export default function ReferralsPage() {
  const [codes, setCodes] = useState<ReferralCode[]>([]);
  const [attribution, setAttribution] = useState<AttributionResponse | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [minting, setMinting] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [redeemValue, setRedeemValue] = useState("");
  const [redeeming, setRedeeming] = useState(false);

  const loadData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [codesRes, attrRes] = await Promise.all([
        request<CodesResponse>("/api/saas/referrals/codes"),
        request<AttributionResponse>("/api/saas/referrals/attribution"),
      ]);
      setCodes(codesRes.codes ?? []);
      setAttribution(attrRes);
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to load referral data");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadData());
  }, [loadData]);

  const mintCode = useCallback(async () => {
    setMinting(true);
    setNotice(null);
    setError(null);
    try {
      const res = await request<{ code: string }>("/api/saas/referrals/codes", {
        method: "POST",
      });
      setNotice(`Code minted: ${res.code}`);
      await loadData();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to mint a referral code");
    } finally {
      setMinting(false);
    }
  }, [loadData]);

  const disableCode = useCallback(
    async (code: string) => {
      setNotice(null);
      setError(null);
      try {
        await request(`/api/saas/referrals/codes/${encodeURIComponent(code)}/disable`, {
          method: "POST",
        });
        setNotice(`Code ${code} disabled.`);
        await loadData();
      } catch (err: unknown) {
        setError(err instanceof Error ? err.message : `Failed to disable ${code}`);
      }
    },
    [loadData],
  );

  const redeem = useCallback(async () => {
    const code = redeemValue.trim();
    if (!code) return;
    setRedeeming(true);
    setNotice(null);
    setError(null);
    try {
      await request("/api/saas/referrals/redeem", {
        method: "POST",
        body: { code },
      });
      setNotice("Referral attributed — thanks for joining through a partner!");
      setRedeemValue("");
      await loadData();
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : "Failed to redeem the code");
    } finally {
      setRedeeming(false);
    }
  }, [redeemValue, loadData]);

  const referredBy = attribution?.referred_by ?? null;
  const referredOrgs = attribution?.referred_organizations ?? [];

  return (
    <AppShell title="Referrals">
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
          marginBottom: "1.5rem",
        }}
      >
        <div>
          <h1 style={{ margin: 0 }}>Referrals</h1>
          <p style={{ margin: "0.25rem 0 0", ...mutedStyle }}>
            Share your code, track who joined through it, and redeem codes you
            received. Attribution is permanent once recorded.
          </p>
        </div>
        <button
          onClick={() => void loadData()}
          className="btn btn-secondary"
          style={{ fontSize: "0.85rem" }}
        >
          Refresh
        </button>
      </div>

      {error && <ErrorState error={error} onRetry={() => void loadData()} />}
      {notice && (
        <div style={{ ...cardStyle, borderColor: "var(--brand-accent, #22c55e)" }}>
          {notice}
        </div>
      )}

      {loading ? (
        <div style={cardStyle}>Loading referral data…</div>
      ) : (
        <>
          {/* Inbound attribution */}
          <div style={cardStyle}>
            <h2 style={{ marginTop: 0 }}>Who referred this organization</h2>
            {referredBy ? (
              <p style={{ margin: 0 }}>
                Referred by <strong>{referredBy.referred_by}</strong> on{" "}
                {new Date(referredBy.attributed_at).toLocaleDateString()}. Attribution
                is permanent and cannot be changed.
              </p>
            ) : (
              <>
                <p style={{ margin: "0 0 0.75rem", ...mutedStyle }}>
                  This organization has not been attributed to a referral code yet.
                  If a partner gave you a code, redeem it here — this can only be
                  done once.
                </p>
                <div style={{ display: "flex", gap: "0.5rem", alignItems: "center" }}>
                  <input
                    value={redeemValue}
                    onChange={(e) => setRedeemValue(e.target.value)}
                    placeholder="REFERRAL CODE"
                    aria-label="Referral code"
                    style={{
                      padding: "0.5rem 0.75rem",
                      borderRadius: "0.375rem",
                      border: "1px solid var(--border, #e2e8f0)",
                      minWidth: "220px",
                    }}
                  />
                  <button
                    onClick={() => void redeem()}
                    disabled={redeeming || redeemValue.trim().length === 0}
                    className="btn btn-primary"
                    style={{ fontSize: "0.85rem" }}
                  >
                    {redeeming ? "Redeeming…" : "Redeem code"}
                  </button>
                </div>
              </>
            )}
          </div>

          {/* Outbound: our codes */}
          <div style={cardStyle}>
            <div
              style={{
                display: "flex",
                justifyContent: "space-between",
                alignItems: "center",
                marginBottom: "0.75rem",
              }}
            >
              <h2 style={{ margin: 0 }}>Your referral codes</h2>
              <button
                onClick={() => void mintCode()}
                disabled={minting}
                className="btn btn-primary"
                style={{ fontSize: "0.85rem" }}
              >
                {minting ? "Minting…" : "Mint new code"}
              </button>
            </div>
            {codes.length === 0 ? (
              <p style={{ margin: 0, ...mutedStyle }}>
                No referral codes yet. Mint one to start referring other
                organizations.
              </p>
            ) : (
              <table style={{ width: "100%", borderCollapse: "collapse", fontSize: "0.9rem" }}>
                <thead>
                  <tr style={{ textAlign: "left" }}>
                    <th style={{ padding: "0.4rem 0.5rem" }}>Code</th>
                    <th style={{ padding: "0.4rem 0.5rem" }}>Status</th>
                    <th style={{ padding: "0.4rem 0.5rem" }}>Redemptions</th>
                    <th style={{ padding: "0.4rem 0.5rem" }}>Created</th>
                    <th style={{ padding: "0.4rem 0.5rem" }} />
                  </tr>
                </thead>
                <tbody>
                  {codes.map((c) => (
                    <tr key={c.code} style={{ borderTop: "1px solid var(--border, #e2e8f0)" }}>
                      <td style={{ padding: "0.4rem 0.5rem", fontFamily: "monospace" }}>
                        {c.code}
                      </td>
                      <td style={{ padding: "0.4rem 0.5rem" }}>{c.status}</td>
                      <td style={{ padding: "0.4rem 0.5rem" }}>{c.redemptions}</td>
                      <td style={{ padding: "0.4rem 0.5rem" }}>
                        {new Date(c.created_at).toLocaleDateString()}
                      </td>
                      <td style={{ padding: "0.4rem 0.5rem", textAlign: "right" }}>
                        {c.status === "active" && (
                          <button
                            onClick={() => void disableCode(c.code)}
                            className="btn btn-secondary"
                            style={{ fontSize: "0.78rem" }}
                          >
                            Disable
                          </button>
                        )}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>

          {/* Outbound attribution list */}
          <div style={cardStyle}>
            <h2 style={{ marginTop: 0 }}>Organizations you referred</h2>
            {referredOrgs.length === 0 ? (
              <p style={{ margin: 0, ...mutedStyle }}>
                No organizations have redeemed your codes yet.
              </p>
            ) : (
              <table style={{ width: "100%", borderCollapse: "collapse", fontSize: "0.9rem" }}>
                <thead>
                  <tr style={{ textAlign: "left" }}>
                    <th style={{ padding: "0.4rem 0.5rem" }}>Organization</th>
                    <th style={{ padding: "0.4rem 0.5rem" }}>Code</th>
                    <th style={{ padding: "0.4rem 0.5rem" }}>Attributed</th>
                  </tr>
                </thead>
                <tbody>
                  {referredOrgs.map((o) => (
                    <tr
                      key={`${o.slug}-${o.attributed_at}`}
                      style={{ borderTop: "1px solid var(--border, #e2e8f0)" }}
                    >
                      <td style={{ padding: "0.4rem 0.5rem" }}>{o.organization}</td>
                      <td style={{ padding: "0.4rem 0.5rem", fontFamily: "monospace" }}>
                        {o.code}
                      </td>
                      <td style={{ padding: "0.4rem 0.5rem" }}>
                        {new Date(o.attributed_at).toLocaleDateString()}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>

          {/* Payouts: policy, never fabricated numbers */}
          <div style={cardStyle}>
            <h2 style={{ marginTop: 0 }}>Payouts</h2>
            <p style={{ margin: 0, ...mutedStyle }}>
              Referral rewards are governed by the platform&apos;s current referral
              terms and are calculated from recorded attributions and billing
              activity. Attribution recorded above is the source of truth; when a
              payout applies to your account it appears on your invoices. This page
              does not estimate or display projected amounts. Questions?{" "}
              <a href={`mailto:${branding.supportEmail}`}>{branding.supportEmail}</a>.
            </p>
          </div>
        </>
      )}
    </AppShell>
  );
}
