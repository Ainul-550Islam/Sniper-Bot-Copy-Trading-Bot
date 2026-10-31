// @vitest-environment jsdom
import type { ComponentType, ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import SniperPage from "@/app/trading/sniper/page";
import SniperConfigPage from "@/app/trading/sniper/config/page";
import CopyPage from "@/app/trading/copy/page";
import CopyConfigPage from "@/app/trading/copy/config/page";

vi.mock("@/components/AppShell", () => {
  const Shell = ({ children }: { children: ReactNode }) => <main>{children}</main>;
  return { default: Shell, AppShell: Shell };
});
vi.mock("next/link", () => ({
  default: ({ href, children, ...props }: { href: string; children: ReactNode; [key: string]: unknown }) =>
    <a href={href} {...props}>{children}</a>,
}));
vi.mock("next/navigation", () => ({
  useParams: () => ({ positionId: "position-fixture", orderId: "order-fixture", signature: "signature-fixture" }),
  useRouter: () => ({ push: vi.fn(), replace: vi.fn(), refresh: vi.fn(), back: vi.fn(), prefetch: vi.fn() }),
  usePathname: () => "/",
  useSearchParams: () => new URLSearchParams(),
}));

type Mode = "empty" | "populated" | "error" | "pending";
type Component = ComponentType;
const org = "a1111111-1111-4111-8111-111111111111";
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
    return Promise.resolve(new Response(JSON.stringify({ error: "test_refusal", reason: "server rejected the request" }), {
      status: 422,
      headers: { "content-type": "application/json" },
    }));
  }
  if (url.pathname === "/api/tenant/strategies" && method === "POST") {
    const record = {
      id: "strategy-created-1",
      organization_id: org,
      name: String(body?.name ?? "Saved test profile"),
      description: String(body?.description ?? ""),
      module: String(body?.module ?? "sniper"),
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
  } else if (/\/api\/tenant\/(sniper|copy)\/status$/.test(url.pathname)) {
    const moduleFamily = url.pathname.includes("sniper") ? "sniper" : "copy";
    payload = {
      organization_id: org, module: moduleFamily,
      entitlement: { feature: `module.${moduleFamily}`, granted: true },
      tenant_override: null, effective_state: "enabled", runtime: null,
      runtime_detail: "No runtime registered in this deployment",
      controls: { available: false, actions: [], scope: "tenant" },
    };
  } else if (url.pathname === "/api/tenant/sniper/config" || url.pathname === "/api/tenant/copy/config") {
    payload = null;
  } else if (url.pathname === "/api/tenant/copy/leaders") {
    payload = {
      organization_id: org,
      items: mode === "populated" ? [{
        organization_id: org,
        address: "So11111111111111111111111111111111111111112",
        label: "Server fixture",
        status: "active",
        source: "config",
        followed_at: now,
        status_since: now,
        events_seen: 0,
        mirrored: 0,
        rejected: 0,
        last_event_at: null,
        last_slot: null,
        updated_at: now,
      }] : [],
    };
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
  { name: "sniper overview", Page: SniperPage, heading: "Solana AMM & Pump.fun Sniper Bot" },
  { name: "sniper configuration", Page: SniperConfigPage, heading: "Solana Sniper Strategy Configuration" },
  { name: "copy overview", Page: CopyPage, heading: "Real-Time Solana Copy Trading" },
  { name: "copy configuration", Page: CopyConfigPage, heading: "Copy Trading Engine Configuration" },
];

describe("sniper and copy trading pages", () => {
  it.each(pages)("renders the real $name page with the server-backed UI", async ({ Page, heading }) => {
    stubApi("empty");
    render(<Page />);
    expect(await screen.findByRole("heading", { name: heading })).toBeTruthy();
  });

  it("shows honest empty states instead of fabricated leaders or profiles", async () => {
    stubApi("empty");
    const { rerender } = render(<SniperPage />);
    expect(await screen.findByText("No saved profiles.")).toBeTruthy();
    expect(screen.queryByRole("listitem")).toBeNull();
    rerender(<CopyPage />);
    expect(await screen.findByText(/No leader wallets configured yet/)).toBeTruthy();
    expect(screen.queryByText("So11111111111111111111111111111111111111112")).toBeNull();
  });

  it("renders saved configuration only after a successful server response", async () => {
    stubApi("empty");
    render(<SniperConfigPage />);
    const values: Record<string, string> = {
      "Strategy Profile Name": "Night Watch",
      "Entry Amount (SOL)": "0.05",
      "Minimum Liquidity (SOL)": "1.25",
      "Maximum Slippage (BPS)": "250",
      "Take Profit (%)": "40",
      "Stop Loss (%)": "15",
      "Trailing Stop (%)": "10",
      "Auto-sell Timeout (seconds)": "300",
      "Priority Fee (lamports)": "5000",
    };
    for (const [label, value] of Object.entries(values)) {
      fireEvent.change(screen.getByLabelText(label), { target: { value } });
    }
    fireEvent.click(screen.getByRole("button", { name: "Save Sniper Configuration" }));
    expect(await screen.findByText("Sniper strategy saved to the authenticated tenant.")).toBeTruthy();
    expect(requests.some((request) => request.method === "POST" && request.path === "/api/tenant/strategies")).toBe(true);

    mode = "populated";
    const { unmount } = render(<SniperPage />);
    expect(await screen.findByText("Night Watch")).toBeTruthy();
    expect(screen.getByText(/Profiles are stored for this tenant/)).toBeTruthy();
    unmount();
  });

  it("keeps the loading state honest and surfaces a real API refusal", async () => {
    stubApi("pending");
    render(<SniperPage />);
    expect(screen.getByText("Loading profiles…")).toBeTruthy();
    cleanup();

    stubApi("error");
    render(<SniperPage />);
    await waitFor(() => expect(screen.getAllByRole("alert").some((alert) => alert.textContent?.includes("server rejected the request"))).toBe(true));
  });

  it("renders server-provided leader rows and not a built-in example", async () => {
    stubApi("populated");
    render(<CopyPage />);
    expect(await screen.findByText("So11111111111111111111111111111111111111112")).toBeTruthy();
    expect(screen.getByText("Server fixture")).toBeTruthy();
    expect(screen.getByText("active")).toBeTruthy();
    expect(screen.queryByText(/No leader wallets configured yet\./)).toBeNull();
  });
});
