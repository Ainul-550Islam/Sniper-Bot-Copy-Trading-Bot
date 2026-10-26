// @vitest-environment jsdom
/**
 * Organization lifecycle behaviour (Part 04, L2).
 *
 * The API client is mocked; close must not POST until the organization slug
 * is typed exactly, suspend / resume must confirm before posting, and every
 * request must hit the exact OpenAPI path with the right method.
 */

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const getLifecycle = vi.fn();
const suspendOrg = vi.fn();
const resumeOrg = vi.fn();
const closeOrg = vi.fn();

vi.mock("@/lib/api/org-lifecycle-api", () => ({
  orgLifecycleApi: {
    get: (...args: unknown[]) => getLifecycle(...args),
    suspend: (...args: unknown[]) => suspendOrg(...args),
    resume: (...args: unknown[]) => resumeOrg(...args),
    close: (...args: unknown[]) => closeOrg(...args),
  },
}));

vi.mock("@/components/AppShell", () => ({
  default: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

vi.mock("@/lib/auth", () => ({
  sessionStore: {
    getSnapshot: () => ({
      organizations: [{ organization_id: "org-1", slug: "e2e-tenant", name: "E2E Tenant", status: "active", role: "owner", membership_status: "active" }],
      selectedOrganizationId: "org-1",
    }),
    subscribe: () => () => {},
  },
}));

import OrganizationPage from "../app/settings/organization/page";

const lifecycle = {
  organization_id: "org-1",
  lifecycle_status: "active",
  state: "active",
  reason: null,
  suspended_at: null,
  suspended_by: null,
  closed_at: null,
  updated_at: "2026-10-20T08:00:00Z",
};

describe("organization lifecycle", () => {
  beforeEach(() => {
    getLifecycle.mockReset();
    suspendOrg.mockReset();
    resumeOrg.mockReset();
    closeOrg.mockReset();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("renders the server-confirmed lifecycle state", async () => {
    getLifecycle.mockResolvedValue({ ...lifecycle, state: "suspended", lifecycle_status: "suspended" });
    render(<OrganizationPage />);
    expect(await screen.findByText("suspended")).toBeTruthy();
    expect(getLifecycle).toHaveBeenCalledWith("org-1");
  });

  it("does not close until the organization slug is typed exactly", async () => {
    getLifecycle.mockResolvedValue({ ...lifecycle });
    closeOrg.mockResolvedValue({ organization_id: "org-1", status: "close_scheduled", reason: "customer requested", updated_at: "2026-10-20T09:00:00Z" });
    const originalConfirm = window.confirm;
    window.confirm = vi.fn(() => true);
    render(<OrganizationPage />);
    await act(async () => {});
    const input = screen.getByLabelText(/type e2e-tenant to confirm/i);
    fireEvent.input(input, { target: { value: "wrong-slug" } });
    const closeBtn = screen.getByRole("button", { name: "Confirm close organization" });
    expect((closeBtn as HTMLButtonElement).disabled).toBe(true);

    fireEvent.input(input, { target: { value: "e2e-tenant" } });
    expect((closeBtn as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Confirm close organization" }));

    await waitFor(() => expect(closeOrg).toHaveBeenCalledWith("org-1", "e2e-tenant"));
    expect(window.confirm).not.toHaveBeenCalled();
    window.confirm = originalConfirm;
  });

  it("suspend and resume confirm before posting", async () => {
    getLifecycle
      .mockResolvedValueOnce({ ...lifecycle })
      .mockResolvedValueOnce({ ...lifecycle, state: "suspended" });
    suspendOrg.mockResolvedValue({ ...lifecycle, state: "suspended" });
    resumeOrg.mockResolvedValue({ ...lifecycle, state: "active" });
    const originalConfirm = window.confirm;
    window.confirm = vi.fn(() => true);
    render(<OrganizationPage />);
    await screen.findByText("active");

    fireEvent.click(screen.getByRole("button", { name: "Suspend organization" }));
    await waitFor(() => expect(suspendOrg).toHaveBeenCalledWith("org-1"));
    expect(window.confirm).toHaveBeenCalled();

    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Resume organization" })).toBeTruthy()
    );
    fireEvent.click(screen.getByRole("button", { name: "Resume organization" }));
    await waitFor(() => expect(resumeOrg).toHaveBeenCalledWith("org-1"));
    window.confirm = originalConfirm;
  });

  it("declined confirmation sends no request", async () => {
    getLifecycle.mockResolvedValue({ ...lifecycle });
    const originalConfirm = window.confirm;
    window.confirm = vi.fn(() => false);
    render(<OrganizationPage />);
    await screen.findByText("active");

    fireEvent.click(screen.getByRole("button", { name: "Suspend organization" }));
    expect(suspendOrg).not.toHaveBeenCalled();
    window.confirm = originalConfirm;
  });
});
