"use client";

import { AssetExposure } from "@/lib/api/portfolio-api";
import { formatBps, formatLamports, formatUsdCents } from "@/lib/formatters/financial";

interface ExposureTableProps {
  exposures: AssetExposure[];
}

export function ExposureTable({ exposures }: ExposureTableProps) {
  if (exposures.length === 0) {
    return (
      <div className="card" style={{ textAlign: "center", padding: "2rem" }}>
        <p style={{ margin: 0, color: "var(--muted)" }}>No asset exposures or active positions held.</p>
      </div>
    );
  }

  return (
    <div className="card" style={{ padding: 0, overflow: "hidden" }}>
      <div style={{ padding: "1rem", borderBottom: "1px solid var(--line)" }}>
        <h3 style={{ margin: 0 }}>Asset Allocation &amp; Venue Exposures</h3>
      </div>

      <table className="table" style={{ width: "100%", borderCollapse: "collapse" }}>
        <thead>
          <tr style={{ background: "rgba(255,255,255,0.02)", textAlign: "left" }}>
            <th style={{ padding: "0.75rem 1rem" }}>Asset / Currency</th>
            <th style={{ padding: "0.75rem 1rem" }}>Holding Quantity</th>
            <th style={{ padding: "0.75rem 1rem" }}>Valuation (USD)</th>
            <th style={{ padding: "0.75rem 1rem" }}>Portfolio Share</th>
            <th style={{ padding: "0.75rem 1rem" }}>Custody / Venue</th>
          </tr>
        </thead>
        <tbody>
          {exposures.map((exp) => (
            <tr key={`${exp.asset_symbol}-${exp.venue}`} style={{ borderTop: "1px solid var(--line)" }}>
              <td style={{ padding: "0.75rem 1rem" }}>
                <strong>{exp.asset_symbol}</strong>
              </td>
              <td style={{ padding: "0.75rem 1rem", fontFamily: "var(--mono)", fontSize: "0.85rem" }}>
                {exp.amount_lamports ? formatLamports(exp.amount_lamports) : `${exp.amount_units.toLocaleString()} ${exp.asset_symbol}`}
              </td>
              <td style={{ padding: "0.75rem 1rem", fontFamily: "var(--mono)", fontSize: "0.9rem" }}>
                {formatUsdCents(exp.value_usd_cents)}
              </td>
              <td style={{ padding: "0.75rem 1rem" }}>
                <span className="badge" style={{ background: "var(--accent-glow)", color: "var(--accent)" }}>
                  {formatBps(exp.percentage_bps)}
                </span>
              </td>
              <td style={{ padding: "0.75rem 1rem" }}>
                <span className="badge" style={{ textTransform: "uppercase" }}>{exp.venue}</span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export default ExposureTable;
