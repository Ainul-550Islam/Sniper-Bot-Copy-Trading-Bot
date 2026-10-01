"use client";

/**
 * Tenant positions / PnL page (PROMPT 5 §K, file 83).
 */

import PositionTable from "@/components/trading/PositionTable";
import PnlCard from "@/components/trading/PnlCard";

export default function PositionsPage() {
  return (
    <main id="main" className="stack">
      <h1>Positions &amp; PnL</h1>
      <PnlCard />
      <PositionTable />
    </main>
  );
}
