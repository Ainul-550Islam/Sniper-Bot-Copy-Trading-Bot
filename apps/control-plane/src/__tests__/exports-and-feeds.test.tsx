// @vitest-environment jsdom
/**
 * Exports, feeds and wallet-access authorization (Part 04, L5).
 *
 * - The export download flows through the authenticated `request` client
 *   (credentials in headers, never in a URL) and becomes a same-origin blob
 *   download.
 * - Notification preferences are read-only: only GET /api/saas/notifications/preferences exists.
 * - Wallet-access authorization must not POST until `window.confirm` accepts.
 */

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const requestMock = vi.fn<(...args: unknown[]) => unknown>();

vi.mock("@/lib/api", () => ({
  request: (...args: unknown[]) => requestMock(...args),
  ApiError: class ApiError extends Error {
    readonly kind: string;
    readonly reason: string;
    readonly status: number;
    constructor(kind: string, reason: string, status: number) {
      super(`${kind}: ${reason}`);
      this.name = "ApiError";
      this.kind = kind;
      this.reason = reason;
      this.status = status;
    }
  },
}));

vi.mock("@/components/AppShell", () => ({
  default: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

vi.mock("@/lib/auth", () => ({
  sessionStore: {
    getSnapshot: () => ({
      organizations: [{ organization_id: "org-1", slug: "org-1", name: "Org One", status: "active", role: "owner", membership_status: "active" }],
      selectedOrganizationId: "org-1",
    }),
    subscribe: () => () => {},
  },
}));

import { exportsApi } from "@/lib/api/exports-api";
import { notificationsApi } from "@/lib/api/notifications-api";
import { eventsApi } from "@/lib/api/events-api";
import { walletAccessApi } from "@/lib/api/wallet-access-api";

describe("exports and feeds", () => {
  let originalCreateObjectURL: typeof URL.createObjectURL;
  let originalRevokeObjectURL: typeof URL.revokeObjectURL;
  let createObjectURLMock: ReturnType<typeof vi.fn>;
  let revokeObjectURLMock: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    requestMock.mockReset();
    originalCreateObjectURL = URL.createObjectURL;
    originalRevokeObjectURL = URL.revokeObjectURL;
    createObjectURLMock = vi.fn(() => "blob:mock-download");
    revokeObjectURLMock = vi.fn();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (URL as any).createObjectURL = createObjectURLMock;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (URL as any).revokeObjectURL = revokeObjectURLMock;
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (URL as any).createObjectURL = originalCreateObjectURL;
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (URL as any).revokeObjectURL = originalRevokeObjectURL;
  });

  it("export download requests the export path and triggers a client blob download", async () => {
    requestMock.mockResolvedValue({
      organization_id: "org-1",
      kind: "audit",
      generated_at: "2026-10-20T08:00:00Z",
      data: {},
    });

    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, "click");

    await exportsApi.download("audit");

    expect(requestMock).toHaveBeenCalledWith("/api/saas/exports?kind=audit");
    expect(createObjectURLMock).toHaveBeenCalled();
    expect(clickSpy).toHaveBeenCalled();
    expect(revokeObjectURLMock).toHaveBeenCalled();
    clickSpy.mockRestore();
  });

  it("report download requests the report download path", async () => {
    requestMock.mockResolvedValue({ id: "rep_1", kind: "accounting_ledger" });
    await exportsApi.downloadReport("rep_1");
    expect(requestMock).toHaveBeenCalledWith("/api/saas/reports/rep_1/download");
  });

  it("notification preferences are read-only (GET only)", async () => {
    requestMock.mockResolvedValue({
      organization_id: "org-1",
      items: [{ channel: "email", enabled: true, detail: null }],
      updated_at: "2026-10-20T08:00:00Z",
    });
    await notificationsApi.preferences();
    expect(requestMock).toHaveBeenCalledWith("/api/saas/notifications/preferences");
    expect(Object.keys(notificationsApi).sort()).toEqual(["preferences"]);
  });

  it("events feed is fetched from the server", async () => {
    requestMock.mockResolvedValue({ organization_id: "org-1", events: [], count: 0 });
    await eventsApi.list();
    expect(requestMock).toHaveBeenCalledWith("/api/saas/events");
  });

  it("wallet-access authorize posts the exact path with the module body", async () => {
    requestMock.mockResolvedValue({ allowed: true, binding: { id: "bind_1" }, reason: "authorized" });
    await walletAccessApi.authorize("bind_1", { module: "module.sniper" });
    expect(requestMock).toHaveBeenCalledWith(
      "/api/saas/wallet-access/bind_1/authorize",
      { method: "POST", body: { module: "module.sniper" } },
    );
  });

  it("authorize sends no POST when the confirmation is declined", async () => {
    const originalConfirm = window.confirm;
    window.confirm = vi.fn(() => false);

    function ConfirmHarness() {
      return (
        <button
          type="button"
          onClick={() => {
            if (window.confirm("Authorize this wallet binding?")) {
              void requestMock("/api/saas/wallet-access/bind_1/authorize", {
                method: "POST",
                body: { module: "module.sniper" },
              });
            }
          }}
        >
          Authorize
        </button>
      );
    }

    render(<ConfirmHarness />);
    fireEvent.click(screen.getByRole("button", { name: "Authorize" }));
    expect(window.confirm).toHaveBeenCalledTimes(1);
    expect(requestMock).not.toHaveBeenCalled();

    window.confirm = originalConfirm;
  });
});
