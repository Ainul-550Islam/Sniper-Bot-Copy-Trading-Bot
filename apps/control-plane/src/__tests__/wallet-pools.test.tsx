// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const request = vi.fn();

vi.mock("@/lib/api", () => ({
  request: (...args: unknown[]) => request(...args),
}));

import { MemberManager } from "../components/wallet-pools/MemberManager";
import { PoolDetailPanel } from "../components/wallet-pools/PoolDetail";
import { PoolList } from "../components/wallet-pools/PoolList";
import type { CustodySigner } from "../lib/api/custody-api";
import {
  walletPoolsApi,
  type WalletPoolDetail,
  type WalletPoolSummary,
} from "../lib/api/wallet-pools-api";

const pool: WalletPoolDetail = {
  id: "550e8400-e29b-41d4-a716-446655440000",
  organization_id: "550e8400-e29b-41d4-a716-446655440001",
  name: "Primary",
  allocation: "weighted_split",
  status: "active",
  rotation_cursor: 2,
  created_at: "2026-10-25T08:00:00Z",
  members: [
    {
      id: "550e8400-e29b-41d4-a716-446655440002",
      weight: 3,
      status: "active",
      logical_identity: "signer-a",
      public_address: "5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1",
      added_at: "2026-10-25T08:30:00Z",
    },
  ],
};

const summary: WalletPoolSummary = {
  id: pool.id,
  name: pool.name,
  allocation: pool.allocation,
  status: pool.status,
  rotation_cursor: pool.rotation_cursor,
  active_members: 1,
  created_at: pool.created_at,
};

const signer: CustodySigner = {
  id: "550e8400-e29b-41d4-a716-446655440003",
  logical_identity: "signer-b",
  public_address: "7Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1",
  provider_type: "kms",
  status: "active",
  capabilities: ["sign"],
};

describe("Wallet pools", () => {
  beforeEach(() => request.mockReset());
  afterEach(() => cleanup());

  it("calls every SaaS wallet-pool endpoint with the correct method", async () => {
    request.mockResolvedValue({});
    await walletPoolsApi.list();
    await walletPoolsApi.create({ name: "Primary", allocation: "weighted_split" });
    await walletPoolsApi.get(pool.id);
    await walletPoolsApi.update(pool.id, { status: "disabled" });
    await walletPoolsApi.addMember(pool.id, { custody_signer_id: signer.id, weight: 2 });
    await walletPoolsApi.removeMember(pool.id, pool.members[0]!.id);

    expect(request.mock.calls).toEqual([
      ["/api/saas/wallet-pools"],
      ["/api/saas/wallet-pools", { method: "POST", body: { name: "Primary", allocation: "weighted_split" } }],
      [`/api/saas/wallet-pools/${pool.id}`],
      [`/api/saas/wallet-pools/${pool.id}`, { method: "PATCH", body: { status: "disabled" } }],
      [`/api/saas/wallet-pools/${pool.id}/members`, { method: "POST", body: { custody_signer_id: signer.id, weight: 2 } }],
      [`/api/saas/wallet-pools/${pool.id}/members/${pool.members[0]!.id}`, { method: "DELETE" }],
    ]);
  });

  it("creates and selects pools from the list", async () => {
    const onCreate = vi.fn().mockResolvedValue(undefined);
    const onSelect = vi.fn();
    render(
      <PoolList
        pools={[summary]}
        selectedPoolId={null}
        canManage
        creating={false}
        onCreate={onCreate}
        onSelect={onSelect}
      />,
    );

    fireEvent.change(screen.getByLabelText("New pool name"), { target: { value: "Secondary" } });
    fireEvent.change(screen.getByLabelText("Allocation"), { target: { value: "weighted_split" } });
    fireEvent.click(screen.getByRole("button", { name: "Create wallet pool" }));
    await waitFor(() => expect(onCreate).toHaveBeenCalledWith({ name: "Secondary", allocation: "weighted_split" }));

    fireEvent.click(screen.getByRole("button", { name: "View" }));
    expect(onSelect).toHaveBeenCalledWith(summary);
  });

  it("saves pool updates through the detail form", async () => {
    const onUpdate = vi.fn().mockResolvedValue(undefined);
    render(<PoolDetailPanel pool={pool} canManage busy={false} onUpdate={onUpdate} />);
    fireEvent.change(screen.getByLabelText("Status"), { target: { value: "disabled" } });
    fireEvent.click(screen.getByRole("button", { name: "Save pool changes" }));
    await waitFor(() =>
      expect(onUpdate).toHaveBeenCalledWith({
        name: "Primary",
        allocation: "weighted_split",
        status: "disabled",
      }),
    );
  });

  it("adds a signer and confirms before removing a member", async () => {
    const onAdd = vi.fn().mockResolvedValue(undefined);
    const onRemove = vi.fn().mockResolvedValue(undefined);
    render(
      <MemberManager
        pool={pool}
        signers={[signer]}
        canManage
        busy={false}
        onAdd={onAdd}
        onRemove={onRemove}
      />,
    );

    fireEvent.change(screen.getByLabelText("Custody signer"), { target: { value: signer.id } });
    fireEvent.change(screen.getByLabelText("Weight"), { target: { value: "4" } });
    fireEvent.click(screen.getByRole("button", { name: "Add member" }));
    await waitFor(() => expect(onAdd).toHaveBeenCalledWith({ custody_signer_id: signer.id, weight: 4 }));

    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(onRemove).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Keep member" }));
    expect(onRemove).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    fireEvent.click(screen.getByRole("button", { name: "Confirm removal" }));
    await waitFor(() => expect(onRemove).toHaveBeenCalledWith(pool.members[0]));
  });
});
