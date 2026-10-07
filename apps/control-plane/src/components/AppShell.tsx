"use client";

import { useEffect, useState } from "react";
import Link from "next/link";
import { usePathname } from "next/navigation";

import { TenantSwitcher } from "@/components/TenantSwitcher";
import {
  clearSession,
  hasLiveSession,
  logout,
  msUntilExpiry,
  selectOrganization,
  sessionStore,
} from "@/lib/auth";

export interface NavGroup {
  title: string;
  items: Array<{
    href: string;
    label: string;
    badge?: string;
  }>;
}

const COMMERCIAL_NAV: NavGroup[] = [
  {
    title: "Overview",
    items: [
      { href: "/", label: "Dashboard" },
      { href: "/portfolio", label: "Portfolio Command" },
      { href: "/risk", label: "Risk & Safeguards" },
      { href: "/trading", label: "Trading Terminal" },
      { href: "/onboarding", label: "Guided Setup", badge: "Start" },
    ],
  },
  {
    title: "Automated Trading",
    items: [
      { href: "/trading/sniper", label: "Sniper Bot" },
      { href: "/trading/copy", label: "Copy Trading" },
      { href: "/trading/polymarket", label: "Polymarket V3" },
      { href: "/trading/telegram", label: "Telegram Alerts" },
    ],
  },
  {
    title: "Strategy & Intelligence",
    items: [
      { href: "/strategies", label: "Strategy Library" },
      { href: "/backtests", label: "Backtesting Workspace" },
      { href: "/markets", label: "Market Screener" },
      { href: "/analytics", label: "Commercial Analytics" },
    ],
  },
  {
    title: "Execution & Records",
    items: [
      { href: "/trading/orders", label: "Orders Book" },
      { href: "/trading/positions", label: "Positions & PnL" },
      { href: "/trading/executions", label: "Execution Journal" },
      { href: "/alerts", label: "Incident Alerts" },
      { href: "/activity", label: "Activity Timeline" },
      { href: "/reports", label: "Reports & Compliance" },
    ],
  },
  {
    title: "Commercial & Platform",
    items: [
      { href: "/billing", label: "Billing & Invoices" },
      { href: "/pricing", label: "Plan Catalog" },
      { href: "/custody", label: "Custody" },
      { href: "/integrations", label: "Integrations & Feeds" },
      { href: "/status", label: "System Status" },
      { href: "/support", label: "Support Tickets" },
      { href: "/docs", label: "Developer Docs" },
    ],
  },
  {
    title: "Organization & Settings",
    items: [
      { href: "/settings/team", label: "Team Members" },
      { href: "/settings/security", label: "Security & Posture" },
      { href: "/settings/api", label: "API Keys" },
      { href: "/settings/webhooks", label: "Webhooks" },
      { href: "/settings/audit", label: "Audit Logs" },
      { href: "/settings/data-lifecycle", label: "Data Lifecycle & Purge" },
    ],
  },
];

/** Live countdown of the in-memory session's remaining life. */
function SessionBadge() {
  const [remaining, setRemaining] = useState(msUntilExpiry());
  useEffect(() => {
    const timer = setInterval(() => setRemaining(msUntilExpiry()), 1000);
    return () => clearInterval(timer);
  }, []);
  const minutes = Math.floor(remaining / 60000);
  const seconds = Math.floor((remaining % 60000) / 1000);
  return (
    <span className="badge" title="Time until the session expires">
      session {minutes}:{String(seconds).padStart(2, "0")}
    </span>
  );
}

export function AppShell({ children }: { children?: React.ReactNode; title?: string }) {
  const pathname = usePathname();
  const state = sessionStore.getSnapshot();
  const [, forceRender] = useState(0);
  const [expired, setExpired] = useState(false);

  useEffect(() => sessionStore.subscribe(() => forceRender((n) => n + 1)), []);

  // Session expiry watch: when the 12-hour window passes, tear down locally.
  useEffect(() => {
    const timer = setInterval(() => {
      if (hasLiveSession() === false) {
        setExpired(true);
        clearSession("expired");
      }
    }, 5000);
    return () => clearInterval(timer);
  }, []);

  if (expired || !hasLiveSession()) {
    return (
      <main id="main" className="landing">
        <div className="card">
          <h1>Session ended</h1>
          <p className="muted">
            Your session expired or was revoked. Reload to sign in again.
          </p>
          <button className="primary" type="button" onClick={() => window.location.reload()}>
            Back to sign-in
          </button>
        </div>
      </main>
    );
  }

  const user = state.user;
  if (!user) return null;

  return (
    <div className="shell">
      <header className="shell__header">
        <div className="shell__brand">
          <Link href="/" style={{ display: "flex", alignItems: "center", gap: "0.5rem", color: "inherit", textDecoration: "none" }}>
            {/* eslint-disable-next-line @next/next/no-img-element */}
            <img src="/logo.svg" alt="" width={28} height={28} />
            <span>Sniper Suite</span>
          </Link>
          <span className="tag tag--active" style={{ fontSize: "0.7rem", padding: "0.1rem 0.4rem" }}>Enterprise</span>
        </div>
        <TenantSwitcher
          organizations={state.organizations}
          selected={state.selectedOrganizationId}
          onSelect={selectOrganization}
        />
        <div className="shell__session">
          <SessionBadge />
          <button type="button" onClick={() => void logout()}>
            Sign out
          </button>
        </div>
      </header>

      <div className="shell__body">
        <nav aria-label="Sections" className="shell__nav">
          {COMMERCIAL_NAV.map((group) => (
            <div key={group.title}>
              <h3>{group.title}</h3>
              <ul>
                {group.items.map((item) => {
                  const isActive = pathname === item.href;
                  return (
                    <li key={item.href}>
                      <Link
                        href={item.href}
                        className={isActive ? "active" : ""}
                        aria-current={isActive ? "page" : undefined}
                      >
                        <span>{item.label}</span>
                        {item.badge && <span className="tag tag--paper" style={{ marginLeft: "auto", fontSize: "0.68rem" }}>{item.badge}</span>}
                      </Link>
                    </li>
                  );
                })}
              </ul>
            </div>
          ))}
        </nav>

        <main id="main" className="shell__main">
          {children ? (
            <article>{children}</article>
          ) : (
            <DashboardContent user={user} />
          )}
        </main>
      </div>
    </div>
  );
}

function DashboardContent({
  user,
}: {
  user: NonNullable<ReturnType<typeof sessionStore.getSnapshot>["user"]>;
}) {
  const state = sessionStore.getSnapshot();
  const currentOrg = state.organizations.find(
    (o) => o.organization_id === state.selectedOrganizationId,
  );

  return (
    <article className="stack">
      <div className="row-between">
        <div>
          <h1>Commercial Control Center</h1>
          <p className="muted">
            Tenant control plane for strategy configuration, multi-venue routing, and configured custody providers.
          </p>
        </div>
        <div className="row">
          <Link href="/onboarding">
            <button className="primary">Guided Onboarding →</button>
          </Link>
          <Link href="/trading">
            <button>Open Trading Desk</button>
          </Link>
        </div>
      </div>

      <div className="grid-3">
        <div className="card">
          <h4>Authenticated User</h4>
          <p style={{ margin: "0.4rem 0 0.2rem", fontSize: "1.05rem", fontWeight: 600 }}>{user.display_name}</p>
          <p className="muted small" style={{ margin: 0 }}>{user.email}</p>
          <div style={{ marginTop: "0.8rem", display: "flex", gap: "0.4rem" }}>
            <span className={`tag tag--${user.status}`}>{user.status}</span>
            {user.platform_admin && <span className="tag tag--active">Platform Admin</span>}
          </div>
        </div>

        <div className="card">
          <h4>Active Tenant Organization</h4>
          {currentOrg ? (
            <>
              <p style={{ margin: "0.4rem 0 0.2rem", fontSize: "1.05rem", fontWeight: 600 }}>{currentOrg.name}</p>
              <p className="muted small" style={{ margin: 0 }}>Slug: <code>{currentOrg.slug}</code></p>
              <div style={{ marginTop: "0.8rem", display: "flex", gap: "0.4rem" }}>
                <span className={`tag tag--${currentOrg.status}`}>{currentOrg.status}</span>
                <span className="badge">Role: {currentOrg.role}</span>
              </div>
            </>
          ) : (
            <p className="muted small" style={{ marginTop: "0.5rem" }}>
              No organization selected. Choose or create an organization in the header switcher.
            </p>
          )}
        </div>

        <div className="card">
          <h4>Execution Environment</h4>
          <p style={{ margin: "0.4rem 0 0.2rem", fontSize: "1.05rem", fontWeight: 600 }}>Solana Routing</p>
          <p className="muted small" style={{ margin: 0 }}>Yellowstone Geyser · Raydium v4 · Pump.fun</p>
          <div style={{ marginTop: "0.8rem", display: "flex", gap: "0.4rem" }}>
            <span className="tag">Deployment health is reported by status</span>
          </div>
        </div>
      </div>

      <div className="card">
        <h2>Quick Navigation &amp; Workspaces</h2>
        <div className="grid-4" style={{ marginTop: "1rem" }}>
          <Link href="/trading/sniper" style={{ textDecoration: "none" }}>
            <div className="card" style={{ background: "var(--panel-2)", height: "100%" }}>
              <h3>⚡ Sniper Bot</h3>
              <p className="muted small">Raydium and Pump.fun launch detection with configured execution controls.</p>
            </div>
          </Link>
          <Link href="/trading/copy" style={{ textDecoration: "none" }}>
            <div className="card" style={{ background: "var(--panel-2)", height: "100%" }}>
              <h3>👥 Copy Trading</h3>
              <p className="muted small">Real-time target wallet mirroring with automated risk controls.</p>
            </div>
          </Link>
          <Link href="/trading/polymarket" style={{ textDecoration: "none" }}>
            <div className="card" style={{ background: "var(--panel-2)", height: "100%" }}>
              <h3>🔮 Polymarket V3</h3>
              <p className="muted small">Prediction market CLOB order book discovery and mirror execution.</p>
            </div>
          </Link>
          <Link href="/backtests" style={{ textDecoration: "none" }}>
            <div className="card" style={{ background: "var(--panel-2)", height: "100%" }}>
              <h3>📊 Backtesting</h3>
              <p className="muted small">Historical simulation engine with exact fee &amp; slippage modelling.</p>
            </div>
          </Link>
        </div>
      </div>
    </article>
  );
}

export default AppShell;
