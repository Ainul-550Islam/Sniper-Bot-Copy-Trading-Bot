"use client";

import type { TenantBalance } from "@/lib/api/trading-detail-api";

interface BalancesTableProps {
  balances: TenantBalance[];
}

/**
 * Tenant balances: every figure renders exactly what the server returned; a
 * null balance stays "—" and is never estimated client-side.
 */
export function BalancesTable({ balances }: BalancesTableProps) {
  return (
    <section className="card" aria-label="Balances">
      <h2>Balances</h2>
      {balances.length === 0 ? (
        <p className="muted">No balances were returned for this organization.</p>
      ) : (
        <table className="table" style={{ width: "100%", borderCollapse: "collapse", marginTop: "1rem" }}>
          <thead>
            <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
              <th style={{ padding: "0.75rem 1rem" }}>Asset</th>
              <th style={{ padding: "0.75rem 1rem" }}>Symbol</th>
              <th style={{ padding: "0.75rem 1rem" }}>Available</th>
              <th style={{ padding: "0.75rem 1rem" }}>Locked</th>
              <th style={{ padding: "0.75rem 1rem" }}>Value (USD)</th>
              <th style={{ padding: "0.75rem 1rem" }}>Updated At</th>
            </tr>
          </thead>
          <tbody>
            {balances.map((b) => (
              <tr key={`${b.asset}-${b.symbol}`} style={{ borderTop: "1px solid var(--line)" }}>
                <td style={{ padding: "0.75rem 1rem" }}><strong>{b.asset}</strong></td>
                <td style={{ padding: "0.75rem 1rem" }}>{b.symbol}</td>
                <td style={{ padding: "0.75rem 1rem" }}>{b.available != null ? String(b.available) : "—"}</td>
                <td style={{ padding: "0.75rem 1rem" }}>{b.locked != null ? String(b.locked) : "—"}</td>
                <td style={{ padding: "0.75rem 1rem" }}>
                  {b.value_usd_cents != null ? `$${(b.value_usd_cents / 100).toFixed(2)}` : "—"}
                </td>
                <td style={{ padding: "0.75rem 1rem", fontSize: "0.85rem", color: "var(--muted)" }}>{b.updated_at}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <p style={{ marginTop: "1rem", fontSize: "0.8rem", color: "var(--muted)"}}>
        Units and values are server truth; no client-side conversion or estimate
        is applied to the displayed numbers.
      </p>
    </section>
  );
}

export default BalancesTable;
