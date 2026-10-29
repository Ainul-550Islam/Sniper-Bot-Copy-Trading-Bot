// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  operatorApi,
  getHealth,
  setMode,
  getModules,
  enableModule,
  disableModule,
  killSystem,
  resumeSystem,
  setRiskKillSwitch,
  getKeys,
  addKey,
  revokeKey,
  getAuditTrail,
  verifyAuditChain,
  getJournalStatus,
  rotateJournal,
  getDbStatus,
  getConfig,
  getReconciliationFindings,
  getRecoveryFailed,
  getExecutions,
  getExecutionById,
  getTrades,
  getWallets,
  getAccountingEvents,
  postAccountingEvent,
  getOrderById,
  getEventsWsPath,
  ApiError,
  OperatorKeyRequiredError,
} from "../lib/api/operator-api";
import { clearSession, establishSession } from "../lib/auth";
import {
  clearOperatorKey,
  getOperatorKey,
  setOperatorKey,
} from "../lib/operator-session";

/** A tenant session is live for every test: operator calls must ignore it. */
const OPERATOR_KEY = "operator-key-material-0123456789abcdef";
const TENANT_TOKEN = "tenant-session-token";

function signIn(): void {
  establishSession({
    token: TENANT_TOKEN,
    user: {
      id: "usr-admin",
      email: "admin@example.test",
      email_verified: true,
      display_name: "Platform Admin",
      status: "active",
      platform_admin: true,
      created_at: new Date().toISOString(),
      last_login_at: null,
    },
    session: {
      id: "ses-1",
      prefix: "ses_1",
      expires_at: new Date(Date.now() + 3_600_000).toISOString(),
      organization_id: "org-1",
    },
  });
}

type FetchCall = [string, RequestInit];

function lastCall(): { url: string; init: RequestInit } {
  const calls = (global.fetch as unknown as { mock: { calls: FetchCall[] } }).mock.calls;
  const call = calls[calls.length - 1] as FetchCall | undefined;
  expect(call, "expected an operator request to have been made").toBeDefined();
  const [url, init] = call as FetchCall;
  return { url, init: init ?? {} };
}

/**
 * The D1 contract, asserted for every single operator function:
 *   * `x-api-key` carries the operator key,
 *   * NO `authorization` header — the tenant session token is a different
 *     credential and the operator plane 401s it,
 *   * NO `x-organization` header — operator routes are not tenant-scoped.
 */
function expectOperatorRequest(options: {
  path: string;
  method?: string;
  body?: unknown;
}): void {
  const { url, init } = lastCall();
  const headers = init.headers as Record<string, string>;

  expect(url).toBe(options.path);
  expect(init.method ?? "GET").toBe(options.method ?? "GET");
  expect(headers["x-api-key"]).toBe(OPERATOR_KEY);
  expect(headers.authorization).toBeUndefined();
  expect(headers["x-organization"]).toBeUndefined();
  expect(JSON.stringify(headers)).not.toContain(TENANT_TOKEN);

  if (options.body !== undefined) {
    expect(init.body).toBe(JSON.stringify(options.body));
  }
}

describe("operator-api client", () => {
  const originalFetch = global.fetch;

  beforeEach(() => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        json: async () => ({ ok: true }),
      }),
    );
    signIn();
    setOperatorKey(OPERATOR_KEY);
  });

  afterEach(() => {
    global.fetch = originalFetch;
    clearOperatorKey();
    clearSession("logout");
    vi.restoreAllMocks();
  });

  it("getHealth calls GET /api/health", async () => {
    await getHealth();
    expectOperatorRequest({ path: "/api/health" });
  });

  it("setMode calls POST /api/mode with payload", async () => {
    await setMode("paper");
    expectOperatorRequest({ path: "/api/mode", method: "POST", body: { mode: "paper" } });
  });

  it("getModules calls GET /api/modules", async () => {
    await getModules();
    expectOperatorRequest({ path: "/api/modules" });
  });

  it("enableModule calls POST /api/modules/:name/enable", async () => {
    await enableModule("sniper");
    expectOperatorRequest({ path: "/api/modules/sniper/enable", method: "POST" });
  });

  it("disableModule calls POST /api/modules/:name/disable", async () => {
    await disableModule("sniper");
    expectOperatorRequest({ path: "/api/modules/sniper/disable", method: "POST" });
  });

  it("killSystem calls POST /api/kill", async () => {
    await killSystem();
    expectOperatorRequest({ path: "/api/kill", method: "POST" });
  });

  it("resumeSystem calls POST /api/resume", async () => {
    await resumeSystem();
    expectOperatorRequest({ path: "/api/resume", method: "POST" });
  });

  it("setRiskKillSwitch calls POST /api/risk/kill-switch", async () => {
    const input = { scope: "venue:polymarket", engaged: true, reason: "incident" };
    await setRiskKillSwitch(input);
    expectOperatorRequest({ path: "/api/risk/kill-switch", method: "POST", body: input });
  });

  it("getKeys calls GET /api/keys", async () => {
    await getKeys();
    expectOperatorRequest({ path: "/api/keys" });
  });

  it("addKey calls POST /api/keys", async () => {
    const input = { label: "test-key", key: "012345678901234567890123456789", role: "operator" };
    await addKey(input);
    expectOperatorRequest({ path: "/api/keys", method: "POST", body: input });
  });

  it("revokeKey calls DELETE /api/keys/:hash", async () => {
    await revokeKey("abc123hash");
    expectOperatorRequest({ path: "/api/keys/abc123hash", method: "DELETE" });
  });

  it("getAuditTrail calls GET /api/audit with optional limit", async () => {
    await getAuditTrail(50);
    expectOperatorRequest({ path: "/api/audit?limit=50" });
  });

  it("verifyAuditChain calls GET /api/audit/verify", async () => {
    await verifyAuditChain();
    expectOperatorRequest({ path: "/api/audit/verify" });
  });

  it("getJournalStatus calls GET /api/journal", async () => {
    await getJournalStatus();
    expectOperatorRequest({ path: "/api/journal" });
  });

  it("rotateJournal calls POST /api/journal", async () => {
    await rotateJournal();
    expectOperatorRequest({ path: "/api/journal", method: "POST" });
  });

  it("getDbStatus calls GET /api/db", async () => {
    await getDbStatus();
    expectOperatorRequest({ path: "/api/db" });
  });

  it("getConfig calls GET /api/config", async () => {
    await getConfig();
    expectOperatorRequest({ path: "/api/config" });
  });

  it("getReconciliationFindings calls GET /api/reconciliation/findings", async () => {
    await getReconciliationFindings(25);
    expectOperatorRequest({ path: "/api/reconciliation/findings?limit=25" });
  });

  it("getRecoveryFailed calls GET /api/recovery/failed", async () => {
    await getRecoveryFailed();
    expectOperatorRequest({ path: "/api/recovery/failed" });
  });

  it("getExecutions calls GET /api/executions with query params", async () => {
    await getExecutions({ limit: 10, state: "pending", open: true });
    expectOperatorRequest({ path: "/api/executions?limit=10&state=pending&open=true" });
  });

  it("getExecutionById calls GET /api/executions/:id", async () => {
    await getExecutionById("exec-123");
    expectOperatorRequest({ path: "/api/executions/exec-123" });
  });

  it("getTrades calls GET /api/trades", async () => {
    await getTrades(20);
    expectOperatorRequest({ path: "/api/trades?limit=20" });
  });

  it("getWallets calls GET /api/wallets", async () => {
    await getWallets();
    expectOperatorRequest({ path: "/api/wallets" });
  });

  it("getAccountingEvents calls GET /api/accounting/events", async () => {
    await getAccountingEvents(15);
    expectOperatorRequest({ path: "/api/accounting/events?limit=15" });
  });

  it("postAccountingEvent calls POST /api/accounting/events", async () => {
    const event = {
      kind: "deposit",
      wallet: "treasury",
      quote_asset: "USDC",
      reference_id: "ref-99",
    };
    await postAccountingEvent(event);
    expectOperatorRequest({ path: "/api/accounting/events", method: "POST", body: event });
  });

  it("getOrderById calls GET /api/orders/:id", async () => {
    await getOrderById("ord-555");
    expectOperatorRequest({ path: "/api/orders/ord-555" });
  });

  it("getEventsWsPath returns /api/events", () => {
    expect(getEventsWsPath()).toBe("/api/events");
  });

  // -----------------------------------------------------------------------
  // D1: the credential contract itself
  // -----------------------------------------------------------------------

  it("every operator function sends x-api-key and neither credential of the tenant plane", async () => {
    const calls: Array<[string, () => Promise<unknown>]> = [
      ["/api/health", () => getHealth()],
      ["/api/mode", () => setMode("paper")],
      ["/api/modules", () => getModules()],
      ["/api/modules/sniper/enable", () => enableModule("sniper")],
      ["/api/modules/sniper/disable", () => disableModule("sniper")],
      ["/api/kill", () => killSystem()],
      ["/api/resume", () => resumeSystem()],
      ["/api/risk/kill-switch", () => setRiskKillSwitch({ scope: "venue:x", engaged: true })],
      ["/api/keys", () => getKeys()],
      ["/api/keys", () => addKey({ label: "l", key: "012345678901234567890123456789", role: "operator" })],
      ["/api/keys/abc", () => revokeKey("abc")],
      ["/api/audit", () => getAuditTrail()],
      ["/api/audit/verify", () => verifyAuditChain()],
      ["/api/journal", () => getJournalStatus()],
      ["/api/journal", () => rotateJournal()],
      ["/api/db", () => getDbStatus()],
      ["/api/config", () => getConfig()],
      ["/api/reconciliation/findings", () => getReconciliationFindings()],
      ["/api/recovery/failed", () => getRecoveryFailed()],
      ["/api/executions", () => getExecutions()],
      ["/api/executions/exec-1", () => getExecutionById("exec-1")],
      ["/api/trades", () => getTrades()],
      ["/api/wallets", () => getWallets()],
      ["/api/accounting/events", () => getAccountingEvents()],
      [
        "/api/accounting/events",
        () =>
          postAccountingEvent({
            kind: "deposit",
            wallet: "treasury",
            quote_asset: "USDC",
            reference_id: "r1",
          }),
      ],
      ["/api/orders/ord-1", () => getOrderById("ord-1")],
    ];

    expect(calls.length).toBe(26);
    for (const [path, call] of calls) {
      await call();
      const { url, init } = lastCall();
      const headers = init.headers as Record<string, string>;
      expect(url, `url for ${path}`).toBe(path);
      expect(headers["x-api-key"], `x-api-key for ${path}`).toBe(OPERATOR_KEY);
      // The tenant plane's credentials must never ride along: `x-api-key` and
      // a session bearer are two different credentials for two different
      // planes, and the operator routes 401 the latter.
      expect(headers.authorization, `authorization for ${path}`).toBeUndefined();
      expect(headers["x-organization"], `x-organization for ${path}`).toBeUndefined();
    }
  });

  it("refuses to build a request when the console is locked", async () => {
    clearOperatorKey();
    await expect(getModules()).rejects.toBeInstanceOf(OperatorKeyRequiredError);
    await expect(killSystem()).rejects.toBeInstanceOf(OperatorKeyRequiredError);
    await expect(getKeys()).rejects.toBeInstanceOf(OperatorKeyRequiredError);
    // Nothing left the browser: a locked console cannot 401 itself.
    expect(global.fetch).not.toHaveBeenCalled();
  });

  it("clears the stored key on a 401 and rethrows the backend ApiError", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: false,
        status: 401,
        statusText: "Unauthorized",
        json: async () => ({ error: "unauthorized", reason: "invalid or missing x-api-key" }),
      }),
    );
    expect(getOperatorKey()).toBe(OPERATOR_KEY);

    try {
      await getModules();
      expect.unreachable("getModules should have thrown");
    } catch (error) {
      const apiError = error as ApiError;
      expect(apiError).toBeInstanceOf(ApiError);
      expect(apiError.status).toBe(401);
      expect(apiError.kind).toBe("unauthorized");
      expect(apiError.reason).toBe("invalid or missing x-api-key");
    }
    // The refused credential is gone, so the UI falls back to the unlock prompt
    // (which surfaces that exact reason) instead of retrying the same 401.
    expect(getOperatorKey()).toBeNull();

    // And a locked console cannot retry either: it fails before the fetch.
    const callsBefore = (global.fetch as unknown as { mock: { calls: unknown[] } }).mock
      .calls.length;
    await expect(getKeys()).rejects.toBeInstanceOf(OperatorKeyRequiredError);
    expect((global.fetch as unknown as { mock: { calls: unknown[] } }).mock.calls.length).toBe(
      callsBefore,
    );
  });

  it("keeps the key on a non-401 refusal (role Insufficient is not a bad key)", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: false,
        status: 403,
        statusText: "Forbidden",
        json: async () => ({
          error: "role_insufficient",
          reason: "role 'readonly' cannot perform 'kill_switch'",
        }),
      }),
    );

    await expect(killSystem()).rejects.toBeInstanceOf(ApiError);
    try {
      await killSystem();
    } catch (error) {
      const apiError = error as ApiError;
      expect(apiError.kind).toBe("role_insufficient");
      expect(apiError.reason).toBe("role 'readonly' cannot perform 'kill_switch'");
      expect(apiError.status).toBe(403);
    }
    expect(getOperatorKey()).toBe(OPERATOR_KEY);
  });

  it("maps API errors into ApiError instances with status and reason", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockResolvedValue({
        ok: false,
        status: 403,
        statusText: "Forbidden",
        json: async () => ({ error: "role_insufficient", reason: "role 'readonly' cannot perform 'kill_switch'" }),
      }),
    );

    await expect(killSystem()).rejects.toThrow(ApiError);
  });

  it("operatorApi object bundles all methods", () => {
    for (const name of [
      "getHealth",
      "setMode",
      "getModules",
      "enableModule",
      "disableModule",
      "killSystem",
      "resumeSystem",
      "setRiskKillSwitch",
      "getKeys",
      "addKey",
      "revokeKey",
      "getAuditTrail",
      "verifyAuditChain",
      "getJournalStatus",
      "rotateJournal",
      "getDbStatus",
      "getConfig",
      "getReconciliationFindings",
      "getRecoveryFailed",
      "getExecutions",
      "getExecutionById",
      "getTrades",
      "getWallets",
      "getAccountingEvents",
      "postAccountingEvent",
      "getOrderById",
      "getEventsWsPath",
    ]) {
      expect(typeof operatorApi[name as keyof typeof operatorApi], name).toBe("function");
    }
  });
});
