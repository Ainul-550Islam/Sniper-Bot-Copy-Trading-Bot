// @vitest-environment jsdom
/**
 * Team invite revoke / resend behaviour (Part 04, L1).
 *
 * The API client is mocked; the page must call the exact resend path with
 * POST, must never send the revoke DELETE until `window.confirm` accepts,
 * and must surface server errors verbatim.
 */

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const listInvites = vi.fn();
const resendInvite = vi.fn();
const revokeInvite = vi.fn();

vi.mock("@/lib/api/team-invites-api", () => ({
  teamInvitesApi: {
    list: (...args: unknown[]) => listInvites(...args),
    resend: (...args: unknown[]) => resendInvite(...args),
    revoke: (...args: unknown[]) => revokeInvite(...args),
  },
}));

vi.mock("@/lib/commercial", () => ({
  commercial: { members: () => ({ members: [] }) },
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

vi.mock("@/components/AppShell", () => ({
  default: ({ children }: { children: React.ReactNode }) => <>{children}</>,
}));

import TeamPage from "../app/settings/team/page";

const invite = {
  id: "inv_1",
  email: "operator@example.test",
  role: "operator",
  status: "pending",
  created_at: "2026-10-20T08:00:00Z",
  expires_at: null,
};

describe("team invites", () => {
  beforeEach(() => {
    listInvites.mockReset();
    resendInvite.mockReset();
    revokeInvite.mockReset();
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("lists pending invites from the team endpoint", async () => {
    listInvites.mockResolvedValue({ organization_id: "org-1", items: [invite], count: 1 });
    render(<TeamPage />);
    expect(await screen.findByText(invite.email)).toBeTruthy();
    expect(listInvites).toHaveBeenCalledTimes(1);
  });

  it("resend posts to the exact resend path", async () => {
    listInvites.mockResolvedValue({ organization_id: "org-1", items: [invite], count: 1 });
    resendInvite.mockResolvedValue({ id: invite.id, status: "resent", resent_at: "2026-10-20T09:00:00Z" });
    render(<TeamPage />);
    await screen.findByText(invite.email);
    fireEvent.click(screen.getByRole("button", { name: "Resend" }));
    await waitFor(() => expect(resendInvite).toHaveBeenCalledWith(invite.id));
    expect(listInvites).toHaveBeenCalledTimes(2);
  });

  it("revoke sends no DELETE until confirmation is accepted", async () => {
    listInvites.mockResolvedValue({ organization_id: "org-1", items: [invite], count: 1 });
    revokeInvite.mockResolvedValue({ id: invite.id, status: "revoked" });
    const originalConfirm = window.confirm;
    window.confirm = vi.fn(() => false);
    render(<TeamPage />);
    await screen.findByText(invite.email);
    fireEvent.click(screen.getByRole("button", { name: "Revoke" }));
    await waitFor(() => expect(window.confirm).toHaveBeenCalledTimes(1));
    expect(revokeInvite).not.toHaveBeenCalled();
    window.confirm = originalConfirm;
  });

  it("revoke deletes after confirmation and refetches invites", async () => {
    listInvites
      .mockResolvedValueOnce({ organization_id: "org-1", items: [invite], count: 1 })
      .mockResolvedValueOnce({ organization_id: "org-1", items: [], count: 0 });
    revokeInvite.mockResolvedValue({ id: invite.id, status: "revoked" });
    const originalConfirm = window.confirm;
    window.confirm = vi.fn(() => true);
    render(<TeamPage />);
    await screen.findByText(invite.email);
    fireEvent.click(screen.getByRole("button", { name: "Revoke" }));
    await waitFor(() => expect(revokeInvite).toHaveBeenCalledWith(invite.id));
    expect(listInvites).toHaveBeenCalledTimes(2);
    expect(await screen.findByText("No pending invitations.")).toBeTruthy();
    window.confirm = originalConfirm;
  });

  it("surfaces a revoke failure verbatim", async () => {
    listInvites.mockResolvedValue({ organization_id: "org-1", items: [invite], count: 1 });
    revokeInvite.mockRejectedValue(new Error("team_store_unavailable: invites are locked"));
    const originalConfirm = window.confirm;
    window.confirm = vi.fn(() => true);
    render(<TeamPage />);
    await screen.findByText(invite.email);
    fireEvent.click(screen.getByRole("button", { name: "Revoke" }));
    await waitFor(() => expect(screen.getByText(/team_store_unavailable/)).toBeTruthy());
    expect(revokeInvite).toHaveBeenCalledWith(invite.id);
    window.confirm = originalConfirm;
  });
});
