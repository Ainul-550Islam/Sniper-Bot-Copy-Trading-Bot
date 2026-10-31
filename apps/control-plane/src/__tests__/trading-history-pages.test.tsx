// @vitest-environment jsdom
import type { ComponentType, ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import TradesPage from "@/app/trading/trades/page";
import TradesForPositionPage from "@/app/trading/trades/[positionId]/page";
import TransactionDetailPage from "@/app/trading/transactions/[signature]/page";
import BalancesPage from "@/app/trading/balances/page";
import RecoveryPage from "@/app/trading/recovery/page";

vi.mock("@/components/AppShell", () => {
  const Shell = ({ children }: { children: ReactNode }) => <main>{children}</main>;
  return { default: Shell, AppShell: Shell };
});
vi.mock("next/link", () => ({
  default: ({ href, children, ...props }: { href: string; children: ReactNode; [key: string]: unknown }) =>
    <a href={href} {...props}>{children}</a>,
}));
vi.mock("next/navigation", () => ({
  useParams: () => ({ positionId: "position-fixture", signature: "signature-fixture" }),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn(), back: vi.fn(), prefetch: vi.fn() }),
  usePathname: () => "/trading",
  useSearchParams: () => new URLSearchParams(),
}));

type Mode = "empty" | "populated" | "error" | "pending";
type Component = ComponentType;
const org = "a3333333-3333-4333-8333-333333333333";
const now = "2026-10-30T00:00:00.000Z";
let mode: Mode = "empty";
let requests: string[] = [];

function apiResponse(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
  const url = new URL(raw, "http://control-plane.test");
  requests.push(`${init?.method ?? "GET"} ${url.pathname}`);
  if (mode === "pending") return new Promise<Response>(() => undefined);
  if (mode === "error") {
    return Promise.resolve(new Response(JSON.stringify({ error: "test_refusal", reason: "history API refused the request" }), {
      status: 422,
      headers: { "content-type": "application/json" },
    }));
  }
  const trade = {
    id: "trade-fixture-001", position_id: "position-fixture", symbol: "FIXTURE", side: "buy",
    qty: 2, price: 12.5, venue: "fixture-venue", status: "filled", executed_at: now,
  };
  let payload: unknown = {};
  if (url.pathname === "/api/tenant/trades") {
    const items = mode === "populated" ? [trade] : [];
    payload = { organization_id: org, items, count: items.length };
  } else if (url.pathname === "/api/tenant/trades/position-fixture") {
    payload = { position_id: "position-fixture", trades: mode === "populated" ? [trade] : [] };
  } else if (url.pathname === "/api/tenant/transactions/signature-fixture") {
    if (mode === "populated") {
      payload = {
        signature: "signature-fixture", status: "confirmed", slot: 123, fee_lamports: 5000,
        confirmed_at: now, memo: "fixture transaction",
      };
    } else {
      return Promise.resolve(Response.json({ error: "not_found", reason: "transaction fixture was not found" }, { status: 404 }));
    }
  } else if (url.pathname === "/api/tenant/balances") {
    const items = mode === "populated" ? [{ asset: "fixture-asset", symbol: "FIX", available: 3.25, locked: 0, value_usd_cents: 4250, updated_at: now }] : [];
    payload = { organization_id: org, items };
  } else if (url.pathname === "/api/tenant/recovery/intents" || url.pathname === "/api/tenant/recovery/sweep") {
    payload = { organization_id: org, items: [] };
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
  { name: "trade history", Page: TradesPage, heading: "Trades" },
  { name: "position trade history", Page: TradesForPositionPage, heading: "Trades for Position: position-fixture" },
  { name: "transaction detail", Page: TransactionDetailPage, heading: "Transaction: signature-fixt…" },
  { name: "balances", Page: BalancesPage, heading: "Balances" },
  { name: "recovery", Page: RecoveryPage, heading: "Recovery" },
];

describe("trading history and detail pages", () => {
  it.each(pages)("renders the real $name page", async ({ Page, heading }) => {
    stubApi("empty");
    render(<Page />);
    expect(await screen.findByRole("heading", { name: heading })).toBeTruthy();
  });

  it("renders an honest empty history and does not invent trades", async () => {
    stubApi("empty");
    render(<TradesPage />);
    expect(await screen.findByText("No trades were returned for this organization.")).toBeTruthy();
    expect(screen.queryByText("FIXTURE")).toBeNull();
    expect(screen.queryByText("trade-fixture-001")).toBeNull();
  });

  it("renders populated tenant rows as server-returned values", async () => {
    stubApi("populated");
    render(<TradesPage />);
    expect(await screen.findByText("FIXTURE")).toBeTruthy();
    expect(screen.getByText("fixture-venue")).toBeTruthy();
    expect(screen.getByText("$12.5000")).toBeTruthy();
    expect(requests).toContain("GET /api/tenant/trades");
  });

  it("preserves loading state and displays a service refusal", async () => {
    stubApi("pending");
    render(<RecoveryPage />);
    expect(screen.getByText("Loading recovery records…")).toBeTruthy();
    cleanup();

    stubApi("error");
    render(<TradesPage />);
    expect(await screen.findByText(/history API refused the request/)).toBeTruthy();
  });
});
