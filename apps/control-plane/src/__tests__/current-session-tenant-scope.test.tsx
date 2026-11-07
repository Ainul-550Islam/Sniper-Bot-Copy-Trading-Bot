// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  snapshot: { session: { id: "session-a" } as { id: string } | null },
  listeners: new Set<() => void>(),
  getUserSessions: vi.fn(),
  revokeUserSession: vi.fn(),
  logout: vi.fn(),
  clearSession: vi.fn(),
}));
vi.mock("@/lib/auth", () => ({
  sessionStore: {
    getSnapshot: () => mocks.snapshot,
    subscribe: (listener: () => void) => {
      mocks.listeners.add(listener);
      return () => mocks.listeners.delete(listener);
    },
  },
  clearSession: (...args: unknown[]) => mocks.clearSession(...args),
}));
vi.mock("@/lib/api", () => ({ auth: { logout: (...args: unknown[]) => mocks.logout(...args) } }));
vi.mock("@/lib/api/security-api", () => ({
  getUserSessions: (...args: unknown[]) => mocks.getUserSessions(...args),
  revokeUserSession: (...args: unknown[]) => mocks.revokeUserSession(...args),
}));

import CurrentSessionPanel from "../components/settings/CurrentSessionPanel";

const item = (id: string, userAgent: string) => ({
  id,
  prefix: `prefix-${id}`,
  current: false,
  state: "active",
  user_agent: userAgent,
  ip: "192.0.2.1",
  last_seen_at: "2026-11-03T00:00:00Z",
  expires_at: "2026-11-04T00:00:00Z",
});

describe("current session inventory scope", () => {
  beforeEach(() => {
    mocks.snapshot = { session: { id: "session-a" } };
    mocks.listeners.clear();
    mocks.getUserSessions.mockReset();
  });
  afterEach(() => cleanup());

  it("clears the previous user's sessions and ignores a late inventory response", async () => {
    let resolveOld!: (value: { items: ReturnType<typeof item>[] }) => void;
    mocks.getUserSessions
      .mockReturnValueOnce(new Promise((resolve) => { resolveOld = resolve; }))
      .mockResolvedValueOnce({ items: [item("session-b", "Browser B")] });
    render(<CurrentSessionPanel />);
    await waitFor(() => expect(mocks.getUserSessions).toHaveBeenCalledTimes(1));

    act(() => {
      mocks.snapshot = { session: { id: "session-b" } };
      mocks.listeners.forEach((listener) => listener());
    });
    expect(await screen.findByText(/Browser B/)).toBeTruthy();
    resolveOld({ items: [item("session-a", "Browser A")] });
    await waitFor(() => expect(screen.getByText(/Browser B/)).toBeTruthy());
    expect(screen.queryByText(/Browser A/)).toBeNull();
  });
});
