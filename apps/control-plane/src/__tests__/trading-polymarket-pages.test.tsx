// @vitest-environment jsdom
import type { ComponentType, ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import PolymarketPage from "@/app/trading/polymarket/page";
import PolymarketConfigPage from "@/app/trading/polymarket/config/page";
import PolymarketOrderPage from "@/app/trading/polymarket/orders/[orderId]/page";

vi.mock("@/components/AppShell", () => {
  const Shell = ({ children }: { children: ReactNode }) => <main>{children}</main>;
  return { default: Shell, AppShell: Shell };
});
vi.mock("next/link", () => ({
  default: ({ href, children, ...props }: { href: string; children: ReactNode; [key: string]: unknown }) =>
    <a href={href} {...props}>{children}</a>,
}));
vi.mock("next/navigation", () => ({
  useParams: () => ({ orderId: "order-fixture" }),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn(), back: vi.fn(), prefetch: vi.fn() }),
  usePathname: () => "/trading/polymarket/orders/order-fixture",
  useSearchParams: () => new URLSearchParams(),
}));

type Mode = "empty" | "populated" | "error" | "pending";
type Component = ComponentType;
const org = "a2222222-2222-4222-8222-222222222222";
const now = "2026-10-30T00:00:00.000Z";
let mode: Mode = "empty";
let strategies: Array<Record<string, unknown>> = [];
let requests: Array<{ path: string; method: string; body: Record<string, unknown> | null }> = [];

function apiResponse(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
  const url = new URL(raw, "http://control-plane.test");
  const method = init?.method ?? "GET";
  const body = typeof init?.body === "string" ? JSON.parse(init.body) as Record<string, unknown> : null;
  requests.push({ path: url.pathname + url.search, method, body });
  if (mode === "pending") return new Promise<Response>(() => undefined);
  if (mode === "error") {
    return Promise.resolve(new Response(JSON.stringify({ error: "test_refusal", reason: "CLOB API refused the request" }), {
      status: 422,
      headers: { "content-type": "application/json" },
    }));
  }
  if (url.pathname === "/api/tenant/strategies" && method === "POST") {
    const record = {
      id: "poly-strategy-created",
      organization_id: org,
      name: String(body?.name ?? "CLOB profile"),
      description: String(body?.description ?? ""),
      module: "polymarket",
      mode: "paper",
      status: "active",
      version: 1,
      config_json: body?.config ?? {},
      created_at: now,
      updated_at: now,
    };
    strategies = [...strategies, record];
    return Promise.resolve(Response.json(record));
  }
  let payload: unknown = {};
  if (url.pathname === "/api/tenant/strategies") {
    const moduleFamily = url.searchParams.get("module");
    const items = strategies.filter((item) => !moduleFamily || item.module === moduleFamily);
    payload = { organization_id: org, items, count: items.length };
  } else if (url.pathname === "/api/tenant/polymarket/status") {
    payload = {
      organization_id: org, module: "polymarket",
      entitlement: { feature: "module.polymarket", granted: true },
      tenant_override: null, effective_state: "enabled", runtime: null,
      runtime_detail: "No Polymarket runtime registered",
      controls: { available: false, actions: [], scope: "tenant" },
    };
  } else if (url.pathname === "/api/tenant/polymarket/config") {
    payload = null;
  } else if (url.pathname === "/api/tenant/polymarket/orders") {
    payload = {
      organization_id: org,
      items: mode === "populated" ? [{ id: "order-fixture", market: "Fixture market", side: "buy", status: "open", created_at: now }] : [],
      count: mode === "populated" ? 1 : 0,
    };
  } else if (url.pathname === "/api/tenant/polymarket/fills") {
    payload = { organization_id: org, items: [] };
  } else if (url.pathname === "/api/tenant/polymarket/reconciliation") {
    payload = { organization_id: org, drift: [] };
  } else if (url.pathname === "/api/tenant/polymarket/orders/order-fixture") {
    payload = mode === "populated" ? {
      venue_order_id: "order-fixture", market: "Fixture market", side: "buy", status: "open",
      price: 0.51, size: 4, created_at: now,
    } : {};
  }
  return Promise.resolve(Response.json(payload));
}

function stubApi(nextMode: Mode = "empty") {
  mode = nextMode;
  strategies = [];
  requests = [];
  vi.stubGlobal("fetch", vi.fn(apiResponse));
}

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

const pages: Array<{ name: string; Page: Component; heading: string }> = [
  { name: "Polymarket trading desk", Page: PolymarketPage, heading: "Polymarket V3 CLOB Trading Desk" },
  { name: "Polymarket strategy form", Page: PolymarketConfigPage, heading: "Polymarket CLOB Strategy Configuration" },
  { name: "Polymarket order details", Page: PolymarketOrderPage, heading: "Polymarket Order: order-fixture…" },
];

describe("Polymarket trading pages", () => {
  it.each(pages)("renders the real $name page", async ({ Page, heading }) => {
    stubApi("empty");
    render(<Page />);
    expect(await screen.findByRole("heading", { name: heading })).toBeTruthy();
  });

  it("shows the server's empty order state without inventing an order", async () => {
    stubApi("empty");
    render(<PolymarketPage />);
    expect(await screen.findByText(/No Polymarket orders recorded yet/)).toBeTruthy();
    expect(screen.queryByText("Fixture market")).toBeNull();
    expect(screen.queryByText("order-fixture")).toBeNull();
  });

  it("renders a populated authoritative order detail returned by the tenant API", async () => {
    stubApi("populated");
    render(<PolymarketOrderPage />);
    expect(await screen.findByText("Fixture market")).toBeTruthy();
    expect(screen.getAllByText("order-fixture").length).toBeGreaterThan(0);
    expect(screen.getByText("$0.5100")).toBeTruthy();
  });

  it("keeps the profile form in loading state while the API is pending and surfaces 4xx errors", async () => {
    stubApi("pending");
    render(<PolymarketPage />);
    expect(screen.getByText("Loading profiles…")).toBeTruthy();
    cleanup();

    stubApi("error");
    render(<PolymarketOrderPage />);
    expect(await screen.findByText(/CLOB API refused the request/)).toBeTruthy();
  });

  it("posts a validated paper profile to the tenant strategy endpoint", async () => {
    stubApi("empty");
    render(<PolymarketConfigPage />);
    const values: Record<string, string> = {
      "Strategy Profile Name": "CLOB Paper Profile",
      "Maximum Position (USDC)": "25",
      "Maximum Market Exposure (USDC)": "100",
      "Spread Threshold (BPS)": "50",
      "Reprice Interval (seconds)": "10",
    };
    for (const [label, value] of Object.entries(values)) {
      fireEvent.change(screen.getByLabelText(label), { target: { value } });
    }
    fireEvent.change(screen.getByLabelText("Market Condition IDs (one per line)"), { target: { value: "fixture-market-condition" } });
    fireEvent.click(screen.getByRole("button", { name: "Save Polymarket Strategy" }));
    expect(await screen.findByText("Polymarket strategy saved to the authenticated tenant.")).toBeTruthy();
    await waitFor(() => expect(requests.some((request) => request.method === "POST" && request.path === "/api/tenant/strategies")).toBe(true));
    expect(strategies[0]?.module).toBe("polymarket");
  });
});
