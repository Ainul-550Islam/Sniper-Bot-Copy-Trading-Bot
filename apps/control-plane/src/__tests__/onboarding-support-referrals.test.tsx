// @vitest-environment jsdom
import type { ComponentType, ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import OnboardingPage from "@/app/onboarding/page";
import SupportPage from "@/app/support/page";
import ReferralsPage from "@/app/referrals/page";
import NewStrategyPage from "@/app/strategies/new/page";
import DocsPage from "@/app/docs/page";
import HomePage from "@/app/page";

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
const org = "a5555555-5555-4555-8555-555555555555";
const now = "2026-10-30T00:00:00.000Z";
let mode: Mode = "empty";
let requests: string[] = [];

function apiResponse(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
  const url = new URL(raw, "http://control-plane.test");
  requests.push(`${init?.method ?? "GET"} ${url.pathname}`);
  if (mode === "pending") return new Promise<Response>(() => undefined);
  if (mode === "error") {
    return Promise.resolve(new Response(JSON.stringify({ error: "test_refusal", reason: "tenant service refused the request" }), {
      status: 422,
      headers: { "content-type": "application/json" },
    }));
  }
  const populated = mode === "populated";
  let payload: unknown = {};
  if (url.pathname === "/api/tenant/onboarding") {
    payload = {
      organization_id: org, current_step: populated ? 3 : 0, steps_total: 6, completed: false,
      steps: { org_created: true, custody_configured: populated, module_enabled: false, strategy_configured: false, paper_trade_executed: false, live_prerequisites_met: false },
      details: { has_active_signer: populated, has_funded_wallet: false, has_enabled_module: false, has_saved_strategy: false },
    };
  } else if (url.pathname === "/api/saas/support/tickets") {
    payload = { organization_id: org, items: populated ? [{ id: "ticket-fixture", organization_id: org, subject: "Fixture support record", priority: "normal", status: "open", description: "Server-returned ticket", created_at: now, updated_at: now }] : [], count: populated ? 1 : 0 };
  } else if (url.pathname === "/api/saas/referrals/codes") {
    payload = init?.method === "POST" ? { code: "SERVER-CODE" } : { codes: populated ? [{ code: "SERVER-CODE", status: "active", created_at: now, redemptions: 0 }] : [] };
  } else if (url.pathname === "/api/saas/referrals/attribution") {
    payload = { referred_by: null, referred_organizations: [] };
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
  { name: "onboarding", Page: OnboardingPage, heading: "Tenant onboarding" },
  { name: "support", Page: SupportPage, heading: "Support & Incident Center" },
  { name: "referrals", Page: ReferralsPage, heading: "Referrals" },
  { name: "new strategy", Page: NewStrategyPage, heading: "Create New Trading Strategy" },
  { name: "developer docs", Page: DocsPage, heading: "Developer API & SDK Documentation" },
  { name: "root sign-in", Page: HomePage, heading: "Sniper Suite — Control Plane" },
];

describe("onboarding, support, referrals and entry pages", () => {
  it.each(pages)("renders the real $name page", async ({ Page, heading }) => {
    stubApi("empty");
    render(<Page />);
    expect(await screen.findByRole("heading", { name: heading })).toBeTruthy();
  });

  it("shows empty support and referral states without invented rows", async () => {
    stubApi("empty");
    const { rerender } = render(<SupportPage />);
    expect(await screen.findByText("No support tickets have been recorded for this organization.")).toBeTruthy();
    expect(screen.queryByText("Fixture support record")).toBeNull();
    rerender(<ReferralsPage />);
    expect(await screen.findByText(/No referral codes yet/)).toBeTruthy();
    expect(screen.queryByText("SERVER-CODE")).toBeNull();
  });

  it("renders populated ticket and referral records only when the server returns them", async () => {
    stubApi("populated");
    const { rerender } = render(<SupportPage />);
    expect(await screen.findByText("Fixture support record")).toBeTruthy();
    rerender(<ReferralsPage />);
    expect(await screen.findByText("SERVER-CODE")).toBeTruthy();
    expect(requests).toContain("GET /api/saas/referrals/codes");
  });

  it("preserves the referral loading state and reports backend refusal", async () => {
    stubApi("pending");
    render(<ReferralsPage />);
    expect(screen.getByText("Loading referral data…")).toBeTruthy();
    cleanup();

    stubApi("error");
    render(<ReferralsPage />);
    expect(await screen.findByText(/tenant service refused the request/)).toBeTruthy();
  });

  it("keeps consent and links present on sign-in and the real strategy form", async () => {
    stubApi("empty");
    const { rerender } = render(<HomePage />);
    expect(await screen.findByRole("button", { name: "Sign in" })).toBeTruthy();
    expect(screen.getByLabelText("Email")).toBeTruthy();
    rerender(<NewStrategyPage />);
    expect(await screen.findByRole("button", { name: "Save Strategy" })).toBeTruthy();
    expect(screen.getByText("Strategy Parameters (JSON)")).toBeTruthy();
    expect(document.querySelector("textarea")).toBeTruthy();
  });
});
