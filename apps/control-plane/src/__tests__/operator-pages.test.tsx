// @vitest-environment jsdom
import type { ComponentType, ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import ModulesPage from "@/app/operator/modules/page";
import JournalPage from "@/app/operator/journal/page";
import ConfigPage from "@/app/operator/config/page";
import ReconciliationPage from "@/app/operator/reconciliation/page";
import ModePage from "@/app/operator/mode/page";
import KillSwitchPage from "@/app/operator/kill-switch/page";
import RecoveryPage from "@/app/operator/recovery/page";
import { OperatorGuard } from "@/components/operator/OperatorGuard";
import { clearOperatorKey, setOperatorKey } from "@/lib/operator-session";

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
  usePathname: () => "/operator",
  useSearchParams: () => new URLSearchParams(),
}));

type Mode = "empty" | "populated" | "error" | "pending";
type Component = ComponentType;
let mode: Mode = "empty";
let requests: string[] = [];

function apiResponse(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
  const url = new URL(raw, "http://control-plane.test");
  requests.push(`${init?.method ?? "GET"} ${url.pathname}`);
  if (mode === "pending") return new Promise<Response>(() => undefined);
  if (mode === "error") {
    return Promise.resolve(new Response(JSON.stringify({ error: "test_refusal", reason: "operator API refused the request" }), {
      status: 503,
      headers: { "content-type": "application/json" },
    }));
  }
  const populated = mode === "populated";
  let payload: unknown = {};
  if (url.pathname === "/api/modules") {
    payload = populated ? [{
      module: "sniper", enabled: true, running: true, healthy: true, connected: true,
      events_seen: 10, signals_generated: 2, orders_sent: 1, orders_filled: 1,
      orders_failed: 0, orders_rejected_by_risk: 0, realized_pnl: 2.5,
      unrealized_pnl: 0, consecutive_errors: 0, last_error: null, last_heartbeat: null,
      last_event: "fixture event", detail: null,
    }] : [];
  } else if (url.pathname === "/api/journal") {
    payload = { available: false, trades_bytes: 0, positions_bytes: 0, events_bytes: 0 };
  } else if (url.pathname === "/api/config") {
    payload = { mode: "paper", runtime: { status: "ready" }, credentials: "<redacted>" };
  } else if (url.pathname === "/api/reconciliation/findings") {
    payload = { count: populated ? 1 : 0, findings: populated ? [{ finding_id: "finding-fixture-001", kind: "quantity_mismatch", module: "sniper", venue: "fixture", asset: "FIX", expected: 1, actual: 0, detail: "Server reconciliation fixture" }] : [] };
  } else if (url.pathname === "/api/status") {
    payload = { execution_mode: "paper", live_allowed: false, kill_switch: false };
  } else if (url.pathname === "/api/recovery/failed") {
    payload = { available: true, count: populated ? 1 : 0, items: populated ? [{ id: "failed-fixture", module: "sniper", venue: "fixture", error: "Server recovery fixture", attempts: 1, failed_at: "2026-10-30T00:00:00.000Z" }] : [] };
  }
  return Promise.resolve(Response.json(payload));
}

function stubApi(nextMode: Mode = "empty") {
  mode = nextMode;
  requests = [];
  setOperatorKey("operator-test-key");
  vi.stubGlobal("fetch", vi.fn(apiResponse));
}

afterEach(() => {
  cleanup();
  clearOperatorKey();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

const pages: Array<{ name: string; Page: Component; heading: string }> = [
  { name: "modules", Page: ModulesPage, heading: "Bot Modules & Supervisor (/api/modules)" },
  { name: "journal", Page: JournalPage, heading: "Intent Journal Storage (/api/journal)" },
  { name: "runtime config", Page: ConfigPage, heading: "Runtime Configuration (/api/config)" },
  { name: "reconciliation", Page: ReconciliationPage, heading: "Accounting Reconciliation (/api/reconciliation/findings)" },
  { name: "execution mode", Page: ModePage, heading: "Execution Mode Control (/api/mode)" },
  { name: "kill switch", Page: KillSwitchPage, heading: "Emergency Stop & Safeguards (/api/kill)" },
  { name: "recovery queue", Page: RecoveryPage, heading: "Execution Recovery Queue (/api/recovery/failed)" },
];

describe("operator pages and access guard", () => {
  it.each(pages)("renders the real $name page with an operator key", async ({ Page, heading }) => {
    stubApi("empty");
    render(<Page />);
    expect(await screen.findByRole("heading", { name: heading })).toBeTruthy();
  });

  it("renders authoritative empty states without fake module or finding rows", async () => {
    stubApi("empty");
    const { rerender } = render(<ModulesPage />);
    expect(await screen.findByText("No modules registered in this deployment.")).toBeTruthy();
    expect(screen.queryByText("fixture event")).toBeNull();
    rerender(<ReconciliationPage />);
    expect(await screen.findByText("No findings were returned.")).toBeTruthy();
    expect(screen.queryByText("finding-fixture-001")).toBeNull();
  });

  it("renders populated module, reconciliation and recovery records from the API", async () => {
    stubApi("populated");
    const { rerender } = render(<ModulesPage />);
    expect(await screen.findByText("sniper")).toBeTruthy();
    rerender(<ReconciliationPage />);
    expect(await screen.findByText("Server reconciliation fixture")).toBeTruthy();
    rerender(<RecoveryPage />);
    expect(await screen.findByText("Server recovery fixture")).toBeTruthy();
  });

  it("keeps the loading state and shows backend refusal without claiming success", async () => {
    stubApi("pending");
    render(<JournalPage />);
    expect(screen.getByRole("heading", { name: "Intent Journal Storage (/api/journal)" })).toBeTruthy();
    cleanup();

    stubApi("error");
    render(<ModePage />);
    await waitFor(() => expect(screen.getByRole("alert")).toBeTruthy());
    expect(screen.getByRole("alert").textContent).toContain("operator API refused the request");
  });

  it("blocks an unauthenticated operator before mounting pages or making requests", () => {
    clearOperatorKey();
    requests = [];
    vi.stubGlobal("fetch", vi.fn(apiResponse));
    render(<OperatorGuard><div>protected operator data</div></OperatorGuard>);
    expect(screen.getByRole("heading", { name: "Operator Access Required" })).toBeTruthy();
    expect(screen.queryByText("protected operator data")).toBeNull();
    expect(requests).toHaveLength(0);
  });
});
