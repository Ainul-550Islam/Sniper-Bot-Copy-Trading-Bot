"use client";

import type { RecoveryIntent, RecoverySweep } from "@/lib/api/trading-detail-api";

interface RecoveryPanelProps {
  intents: RecoveryIntent[];
  sweeps: RecoverySweep[];
}

/**
 * Recovery surface: intents and sweeps recorded by the custody recovery
 * data plane. All values render server truth; nothing is synthesized.
 */
export function RecoveryPanel({ intents, sweeps }: RecoveryPanelProps) {
  return (
    <section className="card" aria-label="Recovery">
      <h2>Recovery</h2>
      <p className="muted small">
        Custody recovery intents and sweeps for this organization, as recorded
        by the recovery data plane.
      </p>

      <div style={{ marginTop: "1rem" }}>
        <h3 style={{ margin: 0 }}>{intents.length} Recovery Intent{intents.length !== 1 ? "s" : ""}</h3>
        {intents.length === 0 && <p className="muted" style={{ marginTop: "0.5rem" }}>No recovery intents recorded.</p>}
        {intents.length > 0 && (
          <table className="table" style={{ width: "100%", borderCollapse: "collapse", marginTop: "0.5rem" }}>
            <thead>
              <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                <th style={{ padding: "0.75rem 1rem" }}>ID</th>
                <th style={{ padding: "0.75rem 1rem" }}>Intent</th>
                <th style={{ padding: "0.75rem 1rem" }}>State</th>
                <th style={{ padding: "0.75rem 1rem" }}>Reason</th>
                <th style={{ padding: "0.75rem 1rem" }}>Created At</th>
              </tr>
            </thead>
            <tbody>
              {intents.map((i) => (
                <tr key={i.id} style={{ borderTop: "1px solid var(--line)" }}>
                  <td style={{ padding: "0.75rem 1rem" }}><code>{i.id.slice(0, 12)}…</code></td>
                  <td style={{ padding: "0.75rem 1rem" }}>{i.intent}</td>
                  <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{i.state}</span></td>
                  <td style={{ padding: "0.75rem 1rem" }}>{i.reason ?? "—"}</td>
                  <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem", color: "var(--muted)" }}>{i.created_at}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <div style={{ marginTop: "1.5rem" }}>
        <h3 style={{ margin: 0 }}>{sweeps.length} Recovery Sweep{sweeps.length !== 1 ? "s" : ""}</h3>
        {sweeps.length === 0 && <p className="muted" style={{ marginTop: "0.5rem" }}>No recovery sweeps recorded.</p>}
        {sweeps.length > 0 && (
          <table className="table" style={{ width: "100%", borderCollapse: "collapse", marginTop: "0.5rem" }}>
            <thead>
              <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
                <th style={{ padding: "0.75rem 1rem" }}>ID</th>
                <th style={{ padding: "0.75rem 1rem" }}>Signature</th>
                <th style={{ padding: "0.75rem 1rem" }}>State</th>
                <th style={{ padding: "0.75rem 1rem" }}>Amount (USD cents)</th>
                <th style={{ padding: "0.75rem 1rem" }}>Created At</th>
              </tr>
            </thead>
            <tbody>
              {sweeps.map((s) => (
                <tr key={s.id} style={{ borderTop: "1px solid var(--line)" }}>
                  <td style={{ padding: "0.75rem 1rem" }}><code>{s.id.slice(0, 12)}…</code></td>
                  <td style={{ padding: "0.75rem 1rem" }}>{s.signature ?? "—"}</td>
                  <td style={{ padding: "0.75rem 1rem" }}><span className="badge">{s.state}</span></td>
                  <td style={{ padding: "0.75rem 1rem" }}>{s.amount_usd_cents != null ? String(s.amount_usd_cents) : "—"}</td>
                  <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem", color: "var(--muted)" }}>{s.created_at}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>

      <p style={{ marginTop: "1rem", fontSize: "0.8rem", color: "var(--muted)" }}>
        Transaction signatures are public on-chain data shown for display only.
      </p>
    </section>
  );
}

export default RecoveryPanel;
