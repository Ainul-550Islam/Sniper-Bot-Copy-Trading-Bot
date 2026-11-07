// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { WebhookDeliveries } from "@/components/settings/webhook-deliveries";
import { listWebhookDeliveries, listWebhooks } from "@/lib/api/webhook-api";

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("webhook response validation", () => {
  it.each([{}, null, { items: null }, { items: {} }])(
    "rejects a malformed endpoint envelope rather than treating it as an empty list: %j",
    async (payload) => {
      vi.stubGlobal("fetch", vi.fn().mockResolvedValue(Response.json(payload)));
      await expect(listWebhooks()).rejects.toThrow("Invalid webhook endpoints response");
    },
  );

  it.each([{}, null, { items: null }, { items: {} }])(
    "rejects a malformed deliveries envelope: %j",
    async (payload) => {
      vi.stubGlobal("fetch", vi.fn().mockResolvedValue(Response.json(payload)));
      await expect(listWebhookDeliveries("endpoint-test")).rejects.toThrow(
        "Invalid webhook deliveries response",
      );
    },
  );

  it("renders an actionable error instead of crashing on a malformed delivery envelope", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(Response.json({})));
    render(<WebhookDeliveries endpointId="endpoint-test" />);
    expect(await screen.findByText("Webhook deliveries unavailable")).toBeTruthy();
    expect(screen.getByText("Invalid webhook deliveries response")).toBeTruthy();
    expect(screen.queryByText("No deliveries yet")).toBeNull();
    expect(screen.getByRole("button", { name: "Retry" })).toBeTruthy();
  });

  it("preserves a valid empty delivery list", async () => {
    vi.stubGlobal("fetch", vi.fn().mockResolvedValue(Response.json({ items: [], count: 0 })));
    await expect(listWebhookDeliveries("endpoint-test")).resolves.toEqual([]);
  });
});
