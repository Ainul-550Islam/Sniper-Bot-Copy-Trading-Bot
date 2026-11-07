import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ tenantRequest: vi.fn() }));
vi.mock("@/lib/customer-trading-api", () => ({
  tenantRequest: (...args: unknown[]) => mocks.tenantRequest(...args),
}));

import { listBacktests } from "@/lib/api/backtest-api";

describe("backtest API response shape", () => {
  beforeEach(() => mocks.tenantRequest.mockReset());

  it("returns server items when the response contains an array", async () => {
    const items = [{ id: "run-1" }];
    mocks.tenantRequest.mockResolvedValue({ organization_id: "org-1", items, count: 1 });

    await expect(listBacktests()).resolves.toBe(items);
    expect(mocks.tenantRequest).toHaveBeenCalledWith("/api/tenant/backtests");
  });

  it.each([null, {}, { items: null }])("rejects malformed list responses: %s", async (response) => {
    mocks.tenantRequest.mockResolvedValue(response);
    await expect(listBacktests()).rejects.toThrow("Invalid backtests response");
  });
});
