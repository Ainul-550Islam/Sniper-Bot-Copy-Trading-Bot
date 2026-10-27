// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import TeamSettingsPage from "@/app/settings/team/page";

const listInvites = vi.fn();
vi.mock("@/lib/commercial", () => ({
  commercial: { members: vi.fn().mockResolvedValue({ members: [] }) },
}));
vi.mock("@/lib/api/team-invites-api", () => ({
  teamInvitesApi: {
    list: (...args: unknown[]) => listInvites(...args),
    resend: vi.fn(),
    revoke: vi.fn(),
  },
}));
vi.mock("@/components/AppShell", () => ({
  default: ({ children }: { children: React.ReactNode }) => <main>{children}</main>,
}));
vi.mock("@/components/settings/team-table", () => ({
  default: () => <div>Members</div>,
}));

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

describe("team invitations DOM structure", () => {
  it("renders pending invite actions without DOM nesting or hydration warnings", async () => {
    listInvites.mockResolvedValue({
      items: [{
        id: "invite-1",
        email: "pending@example.test",
        role: "viewer",
        status: "pending",
        created_at: "2026-10-27T00:00:00.000Z",
      }],
    });
    const errors: string[] = [];
    const warnings: string[] = [];
    const errorSpy = vi.spyOn(console, "error").mockImplementation((...args) => {
      errors.push(args.map(String).join(" "));
    });
    const warnSpy = vi.spyOn(console, "warn").mockImplementation((...args) => {
      warnings.push(args.map(String).join(" "));
    });

    render(<TeamSettingsPage />);
    expect(await screen.findByText("pending@example.test")).toBeTruthy();
    await waitFor(() => expect(screen.getByRole("button", { name: "Resend" })).toBeTruthy());

    const diagnostic = [...errors, ...warnings].join("\n");
    expect(diagnostic).not.toMatch(/validateDOMNesting|hydration/i);
    expect(errorSpy).toHaveBeenCalledTimes(0);
    expect(warnSpy).toHaveBeenCalledTimes(0);
  });
});
