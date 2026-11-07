// @vitest-environment jsdom
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  listPools: vi.fn(),
  getPool: vi.fn(),
  listSigners: vi.fn(),
  listWallets: vi.fn(),
  selectedOrganizationId: "org-a",
  listeners: new Set<() => void>(),
}));

vi.mock("@/lib/auth", () => ({
  sessionStore: {
    getSnapshot: () => ({
      selectedOrganizationId: mocks.selectedOrganizationId,
      organizations: [
        { organization_id: "org-a", role: "owner" },
        { organization_id: "org-b", role: "viewer" },
      ],
    }),
    subscribe: (listener: () => void) => {
      mocks.listeners.add(listener);
      return () => mocks.listeners.delete(listener);
    },
  },
}));

vi.mock("@/lib/api", () => ({
  toDisplayError: (error: unknown) => error instanceof Error ? error.message : String(error),
  wallets: { list: (...args: unknown[]) => mocks.listWallets(...args) },
}));

vi.mock("@/lib/api/wallet-pools-api", () => ({
  walletPoolsApi: {
    list: (...args: unknown[]) => mocks.listPools(...args),
    get: (...args: unknown[]) => mocks.getPool(...args),
    create: vi.fn(),
    update: vi.fn(),
    addMember: vi.fn(),
    removeMember: vi.fn(),
  },
}));

vi.mock("@/lib/api/custody-api", () => ({
  custodyApi: { listSigners: (...args: unknown[]) => mocks.listSigners(...args) },
}));

vi.mock("@/lib/api/wallet-access-api", () => ({ walletAccessApi: { authorize: vi.fn() } }));
vi.mock("@/components/AppShell", () => ({ AppShell: ({ children }: { children: React.ReactNode }) => <>{children}</> }));
vi.mock("@/components/wallet-pools/PoolList", () => ({
  PoolList: ({ pools, canManage }: { pools: Array<{ id: string; name: string }>; canManage: boolean }) => (
    <div>
      <span>Manage: {String(canManage)}</span>
      {pools.map((pool) => <span key={pool.id}>{pool.name}</span>)}
    </div>
  ),
}));
vi.mock("@/components/wallet-pools/PoolDetail", () => ({ PoolDetailPanel: () => null }));
vi.mock("@/components/wallet-pools/MemberManager", () => ({ MemberManager: () => null }));

import WalletPoolsPage from "../app/settings/wallet-pools/page";

const poolsFor = (name: string) => [{
  id: `pool-${name}`,
  name,
  allocation: 100,
  status: "active",
  member_count: 0,
}];

describe("wallet pools tenant scope", () => {
  beforeEach(() => {
    mocks.listPools.mockReset();
    mocks.getPool.mockReset();
    mocks.listSigners.mockReset();
    mocks.listWallets.mockReset();
    mocks.listeners.clear();
    mocks.selectedOrganizationId = "org-a";
    mocks.listPools.mockImplementation(() => Promise.resolve({ pools: poolsFor(mocks.selectedOrganizationId === "org-a" ? "alpha" : "beta") }));
    mocks.getPool.mockImplementation((id: string) => Promise.resolve({ id, name: id, allocation: 100, status: "active", members: [] }));
    mocks.listSigners.mockResolvedValue({ signers: [] });
    mocks.listWallets.mockResolvedValue({ wallets: [] });
  });

  afterEach(() => cleanup());

  it("reloads tenant data and recomputes permissions when the selected organization changes", async () => {
    render(<WalletPoolsPage />);
    expect(await screen.findByText("alpha")).toBeTruthy();
    expect(screen.getByText("Manage: true")).toBeTruthy();

    mocks.selectedOrganizationId = "org-b";
    mocks.listeners.forEach((listener) => listener());

    await waitFor(() => {
      expect(screen.getByText("beta")).toBeTruthy();
      expect(screen.getByText("Manage: false")).toBeTruthy();
    });
    expect(screen.queryByText("alpha")).toBeNull();
    expect(mocks.listPools).toHaveBeenCalledTimes(2);
  });
});
