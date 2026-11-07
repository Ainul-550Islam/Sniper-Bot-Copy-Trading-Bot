import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ request: vi.fn() }));
vi.mock("@/lib/api", () => ({
  ApiError: class ApiError extends Error {},
  request: (...args: unknown[]) => mocks.request(...args),
}));

import { tenantRequest } from "@/lib/customer-trading-api";

describe("tenantRequest path guard", () => {
  beforeEach(() => mocks.request.mockReset());

  it("forwards tenant paths", async () => {
    const response = { items: [] };
    mocks.request.mockResolvedValue(response);
    await expect(tenantRequest("/api/tenant/orders?limit=25")).resolves.toBe(response);
    expect(mocks.request).toHaveBeenCalledWith("/api/tenant/orders?limit=25", {});
  });

  it.each([
    "/api/tenant/../ops/kill",
    "/api/tenant/%2e%2e/ops/kill",
    "/api/tenant/a/%2E%2E/%2E%2E/ops/kill",
  ])("rejects a tenant-prefixed path that normalizes outside the tenant namespace: %s", async (path) => {
    await expect(tenantRequest(path)).rejects.toThrow("refusing non-tenant path");
    expect(mocks.request).not.toHaveBeenCalled();
  });

  it("rejects operator and absolute URLs before making a request", async () => {
    await expect(tenantRequest("/api/ops/kill")).rejects.toThrow("refusing non-tenant path");
    await expect(tenantRequest("https://example.test/api/tenant/orders")).rejects.toThrow(
      "refusing non-tenant path",
    );
    expect(mocks.request).not.toHaveBeenCalled();
  });
});
