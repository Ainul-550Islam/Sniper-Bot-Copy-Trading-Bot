"use client";

/**
 * Tenant trading dashboard (PROMPT 5 §K, file 81).
 *
 * Customer-only: every number and status on this page comes from
 * `/api/tenant/*` endpoints through `lib/customer-trading-api.ts`, which
 * refuses to call anything else. No synthetic demo data — each card
 * renders its own loading / empty / error state honestly.
 */

import Link from "next/link";
import RuntimeCard from "@/components/trading/RuntimeCard";
import PnlCard from "@/components/trading/PnlCard";
import ModuleCards from "@/components/trading/ModuleCards";

export default function TradingDashboardPage() {
  return (
    <main id="main" className="stack">
      <h1>Trading</h1>
      <p className="muted">
        Your organization&apos;s trading surfaces. All data is tenant-scoped and served by the
        customer API — operator-global endpoints are not reachable from this page.
      </p>
      <PnlCard />
      <RuntimeCard />
      <ModuleCards />
      <nav className="card">
        <h2>Reports</h2>
        <ul>
          <li>
            <Link href="/trading/orders">Orders</Link> — your order book with server pagination
          </li>
          <li>
            <Link href="/trading/positions">Positions</Link> — open and historical positions
          </li>
          <li>
            <Link href="/trading/executions">Executions</Link> — the execution lifecycle trail
          </li>
          <li>
            <Link href="/trading/sniper">Sniper</Link> ·{" "}
            <Link href="/trading/copy">Copy</Link> ·{" "}
            <Link href="/trading/polymarket">Polymarket</Link> ·{" "}
            <Link href="/trading/telegram">Telegram</Link> — module controls &amp; status
          </li>
        </ul>
      </nav>
    </main>
  );
}
