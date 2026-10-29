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
} from "../lib/api/operator-api";

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
  });

  afterEach(() => {
    global.fetch = originalFetch;
    vi.restoreAllMocks();
  });

  it("getHealth calls GET /api/health", async () => {
    await getHealth();
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/health$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("setMode calls POST /api/mode with payload", async () => {
    await setMode("paper");
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/mode$/),
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ mode: "paper" }),
      }),
    );
  });

  it("getModules calls GET /api/modules", async () => {
    await getModules();
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/modules$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("enableModule calls POST /api/modules/:name/enable", async () => {
    await enableModule("sniper");
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/modules\/sniper\/enable$/),
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("disableModule calls POST /api/modules/:name/disable", async () => {
    await disableModule("sniper");
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/modules\/sniper\/disable$/),
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("killSystem calls POST /api/kill", async () => {
    await killSystem();
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/kill$/),
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("resumeSystem calls POST /api/resume", async () => {
    await resumeSystem();
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/resume$/),
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("setRiskKillSwitch calls POST /api/risk/kill-switch", async () => {
    await setRiskKillSwitch({ scope: "venue:polymarket", engaged: true, reason: "incident" });
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/risk\/kill-switch$/),
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ scope: "venue:polymarket", engaged: true, reason: "incident" }),
      }),
    );
  });

  it("getKeys calls GET /api/keys", async () => {
    await getKeys();
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/keys$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("addKey calls POST /api/keys", async () => {
    await addKey({ label: "test-key", key: "012345678901234567890123456789", role: "operator" });
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/keys$/),
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ label: "test-key", key: "012345678901234567890123456789", role: "operator" }),
      }),
    );
  });

  it("revokeKey calls DELETE /api/keys/:hash", async () => {
    await revokeKey("abc123hash");
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/keys\/abc123hash$/),
      expect.objectContaining({ method: "DELETE" }),
    );
  });

  it("getAuditTrail calls GET /api/audit with optional limit", async () => {
    await getAuditTrail(50);
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/audit\?limit=50$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("verifyAuditChain calls GET /api/audit/verify", async () => {
    await verifyAuditChain();
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/audit\/verify$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("getJournalStatus calls GET /api/journal", async () => {
    await getJournalStatus();
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/journal$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("rotateJournal calls POST /api/journal", async () => {
    await rotateJournal();
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/journal$/),
      expect.objectContaining({ method: "POST" }),
    );
  });

  it("getDbStatus calls GET /api/db", async () => {
    await getDbStatus();
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/db$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("getConfig calls GET /api/config", async () => {
    await getConfig();
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/config$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("getReconciliationFindings calls GET /api/reconciliation/findings", async () => {
    await getReconciliationFindings(25);
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/reconciliation\/findings\?limit=25$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("getRecoveryFailed calls GET /api/recovery/failed", async () => {
    await getRecoveryFailed();
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/recovery\/failed$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("getExecutions calls GET /api/executions with query params", async () => {
    await getExecutions({ limit: 10, state: "pending", open: true });
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/executions\?limit=10&state=pending&open=true$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("getExecutionById calls GET /api/executions/:id", async () => {
    await getExecutionById("exec-123");
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/executions\/exec-123$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("getTrades calls GET /api/trades", async () => {
    await getTrades(20);
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/trades\?limit=20$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("getWallets calls GET /api/wallets", async () => {
    await getWallets();
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/wallets$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("getAccountingEvents calls GET /api/accounting/events", async () => {
    await getAccountingEvents(15);
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/accounting\/events\?limit=15$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("postAccountingEvent calls POST /api/accounting/events", async () => {
    const event = {
      kind: "deposit",
      wallet: "treasury",
      quote_asset: "USDC",
      reference_id: "ref-99",
    };
    await postAccountingEvent(event);
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/accounting\/events$/),
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify(event),
      }),
    );
  });

  it("getOrderById calls GET /api/orders/:id", async () => {
    await getOrderById("ord-555");
    expect(global.fetch).toHaveBeenCalledWith(
      expect.stringMatching(/\/api\/orders\/ord-555$/),
      expect.objectContaining({ method: "GET" }),
    );
  });

  it("getEventsWsPath returns /api/events", () => {
    expect(getEventsWsPath()).toBe("/api/events");
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
    try {
      await killSystem();
    } catch (err) {
      const apiErr = err as ApiError;
      expect(apiErr.kind).toBe("role_insufficient");
      expect(apiErr.reason).toBe("role 'readonly' cannot perform 'kill_switch'");
      expect(apiErr.status).toBe(403);
    }
  });

  it("operatorApi object bundles all methods", () => {
    expect(typeof operatorApi.getHealth).toBe("function");
    expect(typeof operatorApi.setMode).toBe("function");
    expect(typeof operatorApi.getModules).toBe("function");
    expect(typeof operatorApi.enableModule).toBe("function");
    expect(typeof operatorApi.disableModule).toBe("function");
    expect(typeof operatorApi.killSystem).toBe("function");
    expect(typeof operatorApi.resumeSystem).toBe("function");
    expect(typeof operatorApi.setRiskKillSwitch).toBe("function");
    expect(typeof operatorApi.getKeys).toBe("function");
    expect(typeof operatorApi.addKey).toBe("function");
    expect(typeof operatorApi.revokeKey).toBe("function");
    expect(typeof operatorApi.getAuditTrail).toBe("function");
    expect(typeof operatorApi.verifyAuditChain).toBe("function");
    expect(typeof operatorApi.getJournalStatus).toBe("function");
    expect(typeof operatorApi.rotateJournal).toBe("function");
    expect(typeof operatorApi.getDbStatus).toBe("function");
    expect(typeof operatorApi.getConfig).toBe("function");
    expect(typeof operatorApi.getReconciliationFindings).toBe("function");
    expect(typeof operatorApi.getRecoveryFailed).toBe("function");
    expect(typeof operatorApi.getExecutions).toBe("function");
    expect(typeof operatorApi.getExecutionById).toBe("function");
    expect(typeof operatorApi.getTrades).toBe("function");
    expect(typeof operatorApi.getWallets).toBe("function");
    expect(typeof operatorApi.getAccountingEvents).toBe("function");
    expect(typeof operatorApi.postAccountingEvent).toBe("function");
    expect(typeof operatorApi.getOrderById).toBe("function");
    expect(typeof operatorApi.getEventsWsPath).toBe("function");
  });
});
