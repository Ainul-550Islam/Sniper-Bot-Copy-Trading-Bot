// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MemberManager } from "@/components/wallet-pools/MemberManager";
import type { WalletPoolDetail, WalletPoolMember } from "@/lib/api/wallet-pools-api";

const member = (id: string, logicalIdentity: string): WalletPoolMember => ({
  id,
  weight: 1,
  status: "active",
  logical_identity: logicalIdentity,
  public_address: `address-${id}`,
  added_at: "2026-01-01T00:00:00Z",
});
const pool = (id: string, name: string, poolMember: WalletPoolMember): WalletPoolDetail => ({
  id,
  organization_id: "org-1",
  name,
  allocation: "weighted_split",
  status: "active",
  rotation_cursor: 0,
  created_at: "2026-01-01T00:00:00Z",
  members: [poolMember],
});

describe("wallet pool member removal confirmation", () => {
  afterEach(() => cleanup());

  it("does not carry a pending removal confirmation over to a different pool", () => {
    const onRemove = vi.fn().mockResolvedValue(undefined);
    const poolA = pool("pool-a", "Pool A", member("member-a", "signer-a"));
    const poolB = pool("pool-b", "Pool B", member("member-b", "signer-b"));
    const { rerender } = render(
      <MemberManager pool={poolA} signers={[]} canManage busy={false} onAdd={vi.fn()} onRemove={onRemove} />,
    );

    fireEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(screen.getByRole("alertdialog", { name: "Confirm wallet pool member removal" })).toBeTruthy();
    rerender(
      <MemberManager pool={poolB} signers={[]} canManage busy={false} onAdd={vi.fn()} onRemove={onRemove} />,
    );
    expect(screen.queryByRole("alertdialog", { name: "Confirm wallet pool member removal" })).toBeNull();
    expect(onRemove).not.toHaveBeenCalled();
  });
});
