import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ tenantRequest: vi.fn() }));
vi.mock("@/lib/customer-trading-api", () => ({
  tenantRequest: (...args: unknown[]) => mocks.tenantRequest(...args),
}));

import { listMarkets } from "@/lib/api/market-api";

describe("market API response shape", () => {
  beforeEach(() => mocks.tenantRequest.mockReset());

  it("returns market and feed arrays from the server", async () => {
    const response = { items: [], feeds: [], count: 0, fetched_at: "2026-01-01T00:00:00Z", from_cache: false };
    mocks.tenantRequest.mockResolvedValue(response);
    await expect(listMarkets()).resolves.toBe(response);
    expect(mocks.tenantRequest).toHaveBeenCalledWith("/api/tenant/markets");
  });

  it.each([null, {}, { items: [] }, { feeds: [] }])("rejects malformed market payloads: %s", async (response) => {
    mocks.tenantRequest.mockResolvedValue(response);
    await expect(listMarkets()).rejects.toThrow("Invalid markets response");
  });
});
