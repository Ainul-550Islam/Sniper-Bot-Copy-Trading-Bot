"use client";

/**
 * Tenant Positions & Realized / Unrealized PnL Desk (PROMPT 5 §K, file 83 & Commercial Readiness).
 *
 * Real-time mark-to-market positions, portfolio exposure summary,
 * exact numerical precision, and keyset-paginated inventory.
 */

import { AppShell } from "@/components/AppShell";
import PositionTable from "@/components/trading/PositionTable";
import PnlCard from "@/components/trading/PnlCard";

export default function PositionsPage() {
  return (
    <AppShell>
      <div className="stack">
        <div className="row-between">
          <div>
            <h1>Positions &amp; Portfolio Exposure</h1>
            <p className="muted">
              Authoritative on-chain inventory, entry vs mark valuations, and realized double-entry PnL ledger.
            </p>
          </div>
        </div>

        <div className="grid-2">
          <PnlCard />
          <div className="card">
            <h2>Inventory Valuation Rules</h2>
            <p className="muted small" style={{ marginTop: "0.5rem" }}>
              Mark prices are derived from authoritative on-chain AMM pools (Raydium v4 &amp; Pump.fun) and CLOB midpoints.
              All monetary values preserve exact integer atomic representations so ledger reconciliation stays exact.
            </p>
          </div>
        </div>

        <PositionTable />
      </div>
    </AppShell>
  );
}
