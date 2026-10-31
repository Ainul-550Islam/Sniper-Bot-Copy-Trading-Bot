// @vitest-environment jsdom
import type { ComponentType, ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import SecurityPage from "@/app/settings/security/page";
import NotificationsPage from "@/app/settings/notifications/page";
import WebhooksPage from "@/app/settings/webhooks/page";
import WalletPoolsPage from "@/app/settings/wallet-pools/page";
import SsoPage from "@/app/settings/sso/page";
import AuditPage from "@/app/settings/audit/page";
import DataLifecyclePage from "@/app/settings/data-lifecycle/page";

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
  usePathname: () => "/settings",
  useSearchParams: () => new URLSearchParams(),
}));

type Mode = "empty" | "populated" | "error" | "pending";
type Component = ComponentType;
const org = "a6666666-6666-4666-8666-666666666666";
const now = "2026-10-30T00:00:00.000Z";
let mode: Mode = "empty";
let requests: string[] = [];

function apiResponse(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const raw = typeof input === "string" ? input : input instanceof URL ? input.toString() : input.url;
  const url = new URL(raw, "http://control-plane.test");
  requests.push(`${init?.method ?? "GET"} ${url.pathname}${url.search}`);
  if (mode === "pending") return new Promise<Response>(() => undefined);
  if (mode === "error") {
    return Promise.resolve(new Response(JSON.stringify({ error: "test_refusal", reason: "settings API refused the request" }), {
      status: 422,
      headers: { "content-type": "application/json" },
    }));
  }
  const populated = mode === "populated";
  if (url.pathname === "/api/saas/sso/config" && !populated) {
    return Promise.resolve(new Response(JSON.stringify({ error: "not_found", reason: "SSO configuration not found" }), {
      status: 404,
      headers: { "content-type": "application/json" },
    }));
  }
  let payload: unknown = {};
  if (url.pathname === "/api/saas/users/me/sessions") {
    const items = populated ? [{
      id: "session-fixture",
      prefix: "ses_fixture",
      user_agent: "Firefox test browser",
      ip: "203.0.113.21",
      created_at: now,
      last_seen_at: now,
      expires_at: now,
      revoked_at: null,
      state: "active",
      current: true,
    }] : [];
    payload = { items, count: items.length };
  } else if (url.pathname === "/api/saas/security/status") {
    payload = { organization_id: org, mfa_enforced: populated, has_totp_configured: populated, ip_allowlist: populated ? ["192.0.2.0/24"] : [], ip_allowlist_count: populated ? 1 : 0, session_duration_hours: 12, require_signed_commits: false };
  } else if (url.pathname === "/api/saas/notifications/preferences") {
    payload = { organization_id: org, items: populated ? [{ channel: "email", enabled: true, detail: "Security notices" }] : [], updated_at: now };
  } else if (url.pathname === "/api/saas/webhooks") {
    payload = { organization_id: org, items: populated ? [{ id: "webhook-fixture", organization_id: org, url: "https://hooks.example.test/events", description: "Fixture endpoint", events: ["trade.filled"], is_active: true, created_at: now }] : [], count: populated ? 1 : 0 };
  } else if (url.pathname === "/api/saas/wallet-pools") {
    payload = { organization_id: org, pools: populated ? [{ id: "pool-fixture", name: "Server pool", allocation: "round_robin", status: "active", rotation_cursor: 0, active_members: 1, created_at: now }] : [] };
  } else if (url.pathname === "/api/saas/wallet-pools/pool-fixture") {
    payload = { id: "pool-fixture", organization_id: org, name: "Server pool", allocation: "round_robin", status: "active", rotation_cursor: 0, created_at: now, members: [] };
  } else if (url.pathname === "/api/saas/custody/signers") {
    payload = { organization_id: org, signers: [] };
  } else if (url.pathname === "/api/saas/wallet-access") {
    payload = { organization_id: org, wallets: [] };
  } else if (url.pathname === "/api/saas/sso/config") {
    payload = { id: "sso-fixture", organization_id: org, provider_type: "oidc", issuer_url: "https://idp.example.test", client_id: "fixture-client", client_secret_set: true, redirect_uri: "http://localhost/api/saas/sso/callback", allowed_domains: ["example.test"], role_mapping: {}, enforce_sso: false, status: "active" };
  } else if (url.pathname === "/api/saas/exports" && url.searchParams.get("kind") === "audit") {
    payload = { data: { records: populated ? [{ id: "audit-fixture", actor: "tenant-owner", action: "profile.updated", target: "profile-1", at: now }] : [] } };
  } else if (url.pathname === "/api/saas/audit/export") {
    payload = { data: { records: [] } };
  } else if (url.pathname === "/api/saas/users/me") {
    payload = { user: null, organizations: [{ organization_id: org, slug: "settings-test", name: "Settings Test", status: "active", role: "owner", membership_status: "active" }] };
  } else if (url.pathname === `/api/saas/data-lifecycle/${org}/status`) {
    payload = { organization_id: org, organization_status: "active", phase: "operational", custody_revoked: false, sessions_invalidated: false, retention_scheduled: false, purge_eligible_at: null };
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
  { name: "security posture", Page: SecurityPage, heading: "Security Posture & Access Controls" },
  { name: "notifications", Page: NotificationsPage, heading: "Notification Preferences" },
  { name: "webhooks", Page: WebhooksPage, heading: "Outbound Webhooks & Event Relayers" },
  { name: "wallet pools", Page: WalletPoolsPage, heading: "Wallet Pools" },
  { name: "SSO", Page: SsoPage, heading: "Single sign-on (SSO)" },
  { name: "audit", Page: AuditPage, heading: "Compliance & Security Audit Logs" },
  { name: "data lifecycle", Page: DataLifecyclePage, heading: "Data Lifecycle & Privacy Compliance" },
];

describe("organization settings pages", () => {
  it.each(pages)("renders the real $name page", async ({ Page, heading }) => {
    stubApi("empty");
    render(<Page />);
    expect(await screen.findByRole("heading", { name: heading })).toBeTruthy();
  });

  it("shows empty notification, webhook, pool and audit states from empty responses", async () => {
    stubApi("empty");
    const { rerender } = render(<NotificationsPage />);
    expect(await screen.findByText("No notification preferences were returned for this organization.")).toBeTruthy();
    rerender(<WalletPoolsPage />);
    expect(await screen.findByText("No wallet pools recorded for this organization yet.")).toBeTruthy();
    expect(screen.queryByText("Server pool")).toBeNull();
    rerender(<AuditPage />);
    expect(await screen.findByText("No audit events were returned for this tenant.")).toBeTruthy();
  });

  it("renders populated server settings and the actual security posture", async () => {
    stubApi("populated");
    const { rerender } = render(<NotificationsPage />);
    expect(await screen.findByText("email")).toBeTruthy();
    rerender(<WebhooksPage />);
    expect(await screen.findByText("https://hooks.example.test/events")).toBeTruthy();
    rerender(<SecurityPage />);
    expect(await screen.findByText("ses_fixture")).toBeTruthy();
    expect(screen.getByText(/Firefox test browser/)).toBeTruthy();
    expect(screen.queryByText(/server does not expose a session list/)).toBeNull();
  });

  it("keeps wallet pools loading while service calls are pending and surfaces a 4xx refusal", async () => {
    stubApi("pending");
    render(<WalletPoolsPage />);
    expect(screen.getByText("Loading wallet pools…")).toBeTruthy();
    cleanup();

    stubApi("error");
    render(<SecurityPage />);
    const errors = await screen.findAllByText(/settings API refused the request/);
    expect(errors.length).toBeGreaterThan(0);
  });
});
