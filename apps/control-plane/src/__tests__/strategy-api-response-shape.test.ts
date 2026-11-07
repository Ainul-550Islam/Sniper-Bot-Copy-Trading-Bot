import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ tenantRequest: vi.fn() }));
vi.mock("@/lib/customer-trading-api", () => ({
  tenantRequest: (...args: unknown[]) => mocks.tenantRequest(...args),
}));

import { listStrategies } from "@/lib/api/strategy-api";

const serverStrategy = (organizationId: string) => ({
  id: "strategy-1",
  organization_id: organizationId,
  name: "Test strategy",
  description: "",
  module: "sniper",
  mode: "paper",
  status: "active",
  version: 1,
  config_json: { max_buy_sol: 1 },
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
});

describe("strategy API list response shape", () => {
  beforeEach(() => mocks.tenantRequest.mockReset());

  it("normalizes strategy records from the matching organization", async () => {
    mocks.tenantRequest.mockResolvedValue({
      organization_id: "org-1",
      items: [serverStrategy("org-1")],
      count: 1,
    });
    await expect(listStrategies()).resolves.toEqual([expect.objectContaining({
      module_family: "sniper",
      parameters: { max_buy_sol: 1 },
    })]);
  });

  it.each([null, {}, { items: null }])("rejects malformed list responses: %s", async (response) => {
    mocks.tenantRequest.mockResolvedValue(response);
    await expect(listStrategies()).rejects.toThrow("Invalid strategies response");
  });

  it("rejects records whose organization disagrees with the list response", async () => {
    mocks.tenantRequest.mockResolvedValue({
      organization_id: "org-1",
      items: [serverStrategy("org-2")],
      count: 1,
    });
    await expect(listStrategies()).rejects.toThrow("Strategy records do not match the response organization");
  });
});
