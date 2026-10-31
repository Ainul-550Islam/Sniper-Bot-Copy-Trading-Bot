// @vitest-environment jsdom
import type { ComponentType, ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import PortfolioPage from "@/app/portfolio/page";
import ActivityPage from "@/app/activity/page";
import AlertsPage from "@/app/alerts/page";
import AnalyticsPage from "@/app/analytics/page";
import IntegrationsPage from "@/app/integrations/page";

vi.mock("@/components/AppShell", () => {
  const Shell = ({ children }: { children: ReactNode }) => <main>{children}</main>;
  return { default: Shell, AppShell: Shell };
});
vi.mock("next/link", () => ({
  default: ({ href, children, ...props }: { href: string; children: ReactNode; [key: string]: unknown }) =>
    <a href={href} {...props}>{children}</a>,
}));
vi.mock("next/navigation", () => ({
  useParams: () => ({}),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn(), back: vi.fn(), prefetch: vi.fn() }),
  usePathname: () => "/",
  useSearchParams: () => new URLSearchParams(),
}));

type Mode = "empty" | "populated" | "error" | "pending";
type Component = ComponentType;
const org = "a4444444-4444-4444-8444-444444444444";
const now = "2026-10-30T00:00:00.000Z";
let mode: Mode = "empty";
let requests: string[] = [];

function apiResponse(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
  const url = new URL(raw, "http://control-plane.test");
  requests.push(`${init?.method ?? "GET"} ${url.pathname}`);
  if (mode === "pending") return new Promise<Response>(() => undefined);
  if (mode === "error") {
    return Promise.resolve(new Response(JSON.stringify({ error: "test_refusal", reason: "portfolio service refused the request" }), {
      status: 422,
      headers: { "content-type": "application/json" },
    }));
  }
  const populated = mode === "populated";
  let payload: unknown = {};
  if (url.pathname === "/api/saas/portfolio") {
    payload = {
      organization_id: org,
      total_equity_usd_cents: populated ? 125000 : 0,
      available_cash_usd_cents: populated ? 75000 : 0,
      allocated_margin_usd_cents: populated ? 50000 : 0,
      unrealized_pnl_usd_cents: populated ? 2500 : 0,
      realized_pnl_30d_usd_cents: populated ? 1200 : 0,
      max_drawdown_bps: 0,
      exposures: populated ? [{ asset_symbol: "FIX", amount_units: 2, value_usd_cents: 1200, percentage_bps: 960, venue: "fixture" }] : [],
      as_of: now,
      is_stale: false,
    };
  } else if (url.pathname === "/api/saas/activity") {
    payload = { items: populated ? [{ id: "activity-fixture", actor: "tenant-owner", action: "fixture_action", object_type: "trade", object_id: "trade-1", summary: "Server activity fixture", timestamp: now }] : [] };
  } else if (url.pathname === "/api/saas/events") {
    payload = { organization_id: org, events: populated ? [{ id: "event-fixture", actor: "tenant-owner", action: "fixture_event", target: null, summary: "Server event fixture", timestamp: now }] : [], count: populated ? 1 : 0 };
  } else if (url.pathname === "/api/saas/alerts") {
    payload = {
      organization_id: org,
      items: populated ? [{ id: "alert-fixture", organization_id: org, severity: "warning", category: "system", title: "Fixture alert", message: "Server-returned alert", is_acknowledged: false, created_at: now }] : [],
      unacknowledged_count: populated ? 1 : 0,
    };
  } else if (url.pathname === "/api/tenant/analytics") {
    payload = {
      organization_id: org, timeframe: "7d", total_trades: populated ? 1 : 0,
      win_rate_pct: populated ? 100 : null, realized_pnl_usd: populated ? 12.5 : 0,
      unrealized_pnl_usd: 0, total_volume_usd: populated ? 25 : 0, total_fees_usd: 0,
      sharpe_ratio: null, max_drawdown_pct: null,
      pnl_series: populated ? [{ date: "2026-10-30", pnl_usd: 12.5, cumulative_usd: 12.5 }] : [],
      module_breakdown: populated ? [{ module: "sniper", trades_count: 1, volume_usd: 25, pnl_usd: 12.5, win_rate_pct: 100 }] : [],
      execution_quality: { avg_fill_time_ms: null, avg_slippage_bps: null, failed_attempts_pct: 0, reverted_txs: 0 },
    };
  } else if (url.pathname === "/api/tenant/integrations") {
    payload = {
      organization_id: org,
      items: populated ? [{ id: "integration-fixture", organization_id: org, provider_name: "Fixture RPC", category: "solana_rpc", status: "connected", endpoint_url_masked: "https://rpc.example.test/***", evidence_level: "live_verified", last_health_check_at: now, latency_ms: 25 }] : [],
      count: populated ? 1 : 0,
    };
  }
  return Promise.resolve(Response.json(payload));
}

function stubApi(nextMode: Mode = "empty") {
  mode = nextMode;
  requests = [];
  vi.stubGlobal("fetch", vi.fn(apiResponse));
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

const pages: Array<{ name: string; Page: Component; heading: string }> = [
  { name: "portfolio", Page: PortfolioPage, heading: "Portfolio Command Center" },
  { name: "activity", Page: ActivityPage, heading: "Organization Activity Timeline" },
  { name: "alerts", Page: AlertsPage, heading: "Incident & Risk Alerts Center" },
  { name: "analytics", Page: AnalyticsPage, heading: "Trading Analytics" },
  { name: "integrations", Page: IntegrationsPage, heading: "Connected Infrastructure & Ecosystem Integrations" },
];

describe("portfolio, activity, alerts and analytics pages", () => {
  it.each(pages)("renders the real $name page", async ({ Page, heading }) => {
    stubApi("empty");
    render(<Page />);
    expect(await screen.findByRole("heading", { name: heading })).toBeTruthy();
  });

  it("shows real empty states and never manufactures an alert or provider", async () => {
    stubApi("empty");
    const { rerender } = render(<AlertsPage />);
    expect(await screen.findByText("No unacknowledged system or risk alerts.")).toBeTruthy();
    expect(screen.queryByText("Fixture alert")).toBeNull();
    rerender(<IntegrationsPage />);
    expect(await screen.findByText("No integration health components are registered in this deployment.")).toBeTruthy();
    expect(screen.queryByText("Fixture RPC")).toBeNull();
  });

  it("renders server-sourced activity and analytics fixtures only when returned", async () => {
    stubApi("populated");
    const { rerender } = render(<ActivityPage />);
    expect(await screen.findByText("Server activity fixture")).toBeTruthy();
    expect(screen.getByText("Server event fixture")).toBeTruthy();
    rerender(<AnalyticsPage />);
    expect(await screen.findByText("sniper", { selector: "strong" })).toBeTruthy();
    expect(requests).toContain("GET /api/tenant/analytics");
  });

  it("exposes the loading and error states from the actual portfolio fetch", async () => {
    stubApi("pending");
    render(<PortfolioPage />);
    expect(screen.getByText("Loading portfolio projections...")).toBeTruthy();
    cleanup();

    stubApi("error");
    render(<PortfolioPage />);
    expect(await screen.findByText(/portfolio service refused the request/)).toBeTruthy();
  });
});
