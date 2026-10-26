// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const request = vi.fn();

vi.mock("@/lib/api", () => ({
  request: (...args: unknown[]) => request(...args),
}));

import { ProfilePanel } from "../components/custody/ProfilePanel";
import { RotationPanel } from "../components/custody/RotationPanel";
import { SignerPanel } from "../components/custody/SignerPanel";
import {
  custodyApi,
  type CustodyProfile,
  type CustodyRotation,
  type CustodySigner,
} from "../lib/api/custody-api";

const signerA: CustodySigner = {
  id: "550e8400-e29b-41d4-a716-446655440010",
  logical_identity: "signer-a",
  public_address: "5Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1",
  provider_type: "kms",
  status: "active",
  capabilities: ["sign"],
};

const signerB: CustodySigner = {
  ...signerA,
  id: "550e8400-e29b-41d4-a716-446655440011",
  logical_identity: "signer-b",
  public_address: "7Q544fKrFoe6tsEbD7S8EmxGTJYAKtTVhAW5Q5pge4j1",
};

const profile: CustodyProfile = {
  id: "550e8400-e29b-41d4-a716-446655440000",
  organization_id: "550e8400-e29b-41d4-a716-446655440001",
  name: "Production",
  provider_type: "kms",
  status: "pending",
  public_signers: [signerA, signerB],
  created_at: "2026-10-25T08:00:00Z",
};

const rotation: CustodyRotation = {
  id: "550e8400-e29b-41d4-a716-446655440020",
  organization_id: profile.organization_id,
  profile_id: profile.id,
  old_signer: signerA.id,
  new_signer: signerB.id,
  state: "pending",
  force_revoked: false,
  created_at: "2026-10-25T09:00:00Z",
  updated_at: "2026-10-25T09:00:00Z",
};

describe("Custody actions", () => {
  beforeEach(() => {
    request.mockReset();
    window.localStorage.clear();
    window.sessionStorage.clear();
  });
  afterEach(() => cleanup());

  it("calls every custody management endpoint with its documented method", async () => {
    request.mockResolvedValue({});
    await custodyApi.listProfiles();
    await custodyApi.createProfile({ name: "Production", provider_type: "kms" });
    await custodyApi.activateProfile(profile.id);
    await custodyApi.revokeProfile(profile.id);
    await custodyApi.listProfileSigners(profile.id);
    await custodyApi.listSigners();
    await custodyApi.createSigner({
      custody_profile_id: profile.id,
      logical_identity: signerA.logical_identity,
      public_address: signerA.public_address,
      capabilities: ["sign"],
    });
    await custodyApi.getSigner(signerA.id);
    await custodyApi.activateSigner(signerA.id);
    await custodyApi.revokeSigner(signerA.id);
    await custodyApi.attachCapability(signerA.id, { capability: "sign" });
    await custodyApi.resolveSigner(signerA.id);
    await custodyApi.createRotation({
      profile_id: profile.id,
      old_signer_id: signerA.id,
      new_signer_id: signerB.id,
    });
    await custodyApi.getRotation(rotation.id);
    await custodyApi.activateRotation(rotation.id);
    await custodyApi.revokeRotation(rotation.id);
    await custodyApi.signerHealth(signerA.id);

    expect(request.mock.calls).toEqual([
      ["/api/saas/custody/profiles"],
      ["/api/saas/custody/profiles", { method: "POST", body: { name: "Production", provider_type: "kms" } }],
      [`/api/saas/custody/profiles/${profile.id}/activate`, { method: "POST" }],
      [`/api/saas/custody/profiles/${profile.id}/revoke`, { method: "POST" }],
      [`/api/saas/custody/profiles/${profile.id}/signers`],
      ["/api/saas/custody/signers"],
      [
        "/api/saas/custody/signers",
        {
          method: "POST",
          body: {
            custody_profile_id: profile.id,
            logical_identity: signerA.logical_identity,
            public_address: signerA.public_address,
            capabilities: ["sign"],
          },
        },
      ],
      [`/api/saas/custody/signers/${signerA.id}`],
      [`/api/saas/custody/signers/${signerA.id}/activate`, { method: "POST" }],
      [`/api/saas/custody/signers/${signerA.id}/revoke`, { method: "POST" }],
      [`/api/saas/custody/signers/${signerA.id}/capabilities`, { method: "POST", body: { capability: "sign" } }],
      [`/api/saas/custody/signers/${signerA.id}/resolve`],
      [
        "/api/saas/custody/rotations",
        {
          method: "POST",
          body: {
            profile_id: profile.id,
            old_signer_id: signerA.id,
            new_signer_id: signerB.id,
          },
        },
      ],
      [`/api/saas/custody/rotations/${rotation.id}`],
      [`/api/saas/custody/rotations/${rotation.id}/activate`, { method: "POST" }],
      [`/api/saas/custody/rotations/${rotation.id}/revoke`, { method: "POST" }],
      [`/api/saas/custody/health/${signerA.id}`],
    ]);
  });

  it("creates a profile and confirms activation separately", async () => {
    const onCreate = vi.fn().mockResolvedValue(undefined);
    const onActivate = vi.fn().mockResolvedValue(undefined);
    render(
      <ProfilePanel
        profiles={[profile]}
        canManage
        busy={false}
        onCreate={onCreate}
        onActivate={onActivate}
        onRevoke={async () => {}}
      />,
    );

    fireEvent.change(screen.getByLabelText("Profile name"), { target: { value: "Backup" } });
    fireEvent.change(screen.getByLabelText("Provider type"), { target: { value: "vault" } });
    fireEvent.click(screen.getByRole("button", { name: "Create custody profile" }));
    await waitFor(() => expect(onCreate).toHaveBeenCalledWith({ name: "Backup", provider_type: "vault" }));

    fireEvent.click(screen.getByRole("button", { name: "Activate" }));
    expect(onActivate).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Confirm activate" }));
    await waitFor(() => expect(onActivate).toHaveBeenCalledWith(profile));
  });

  it("creates and manages signers using public metadata only", async () => {
    const onCreate = vi.fn().mockResolvedValue(undefined);
    const onRevoke = vi.fn().mockResolvedValue(undefined);
    const onAttach = vi.fn().mockResolvedValue(undefined);
    const onResolve = vi.fn().mockResolvedValue(undefined);
    const onHealth = vi.fn().mockResolvedValue(undefined);
    const onGet = vi.fn().mockResolvedValue(undefined);
    const onLoadProfileSigners = vi.fn().mockResolvedValue(undefined);
    const { container } = render(
      <SignerPanel
        profiles={[profile]}
        signers={[signerA]}
        healthBySigner={{}}
        healthLoadingId={null}
        resolvedSigner={null}
        detailedSigner={null}
        profileSigners={[signerB]}
        profileSignersProfile={profile}
        canManage
        busy={false}
        onCreate={onCreate}
        onActivate={async () => {}}
        onRevoke={onRevoke}
        onAttachCapability={onAttach}
        onResolve={onResolve}
        onHealth={onHealth}
        onGet={onGet}
        onLoadProfileSigners={onLoadProfileSigners}
      />,
    );

    expect(container.querySelector('input[type="password"]')).toBeNull();
    expect(screen.queryByLabelText(/private key/i)).toBeNull();

    fireEvent.change(screen.getByLabelText("Custody profile"), { target: { value: profile.id } });
    fireEvent.change(screen.getByLabelText("Logical identity"), { target: { value: "signer-c" } });
    fireEvent.change(screen.getByLabelText("Public address"), { target: { value: signerB.public_address } });
    fireEvent.change(screen.getByLabelText("Capabilities (comma-separated)"), { target: { value: "sign, verify" } });
    fireEvent.click(screen.getByRole("button", { name: "Create signer" }));
    await waitFor(() =>
      expect(onCreate).toHaveBeenCalledWith({
        custody_profile_id: profile.id,
        logical_identity: "signer-c",
        public_address: signerB.public_address,
        capabilities: ["sign", "verify"],
      }),
    );

    expect(screen.getByText(/signer-b/)).toBeTruthy();
    fireEvent.change(screen.getByLabelText("Signers for profile"), { target: { value: profile.id } });
    fireEvent.click(screen.getByRole("button", { name: "Load profile signers" }));
    await waitFor(() => expect(onLoadProfileSigners).toHaveBeenCalledWith(profile));

    fireEvent.click(screen.getByRole("button", { name: "Details" }));
    fireEvent.click(screen.getByRole("button", { name: "Health" }));
    fireEvent.click(screen.getByRole("button", { name: "Resolve" }));
    expect(onGet).toHaveBeenCalledWith(signerA);
    expect(onHealth).toHaveBeenCalledWith(signerA);
    expect(onResolve).toHaveBeenCalledWith(signerA);

    fireEvent.change(screen.getByLabelText("Attach capability to signer"), { target: { value: signerA.id } });
    fireEvent.change(screen.getByLabelText("Capability"), { target: { value: "rotate" } });
    fireEvent.click(screen.getByRole("button", { name: "Attach capability" }));
    await waitFor(() => expect(onAttach).toHaveBeenCalledWith(signerA, "rotate"));

    fireEvent.click(screen.getByRole("button", { name: "Revoke" }));
    expect(onRevoke).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Confirm revoke" }));
    await waitFor(() => expect(onRevoke).toHaveBeenCalledWith(signerA));

    expect(window.localStorage.length).toBe(0);
    expect(window.sessionStorage.length).toBe(0);
    expect(window.location.search).toBe("");
  });

  it("creates a rotation from the selected active profile's signers", async () => {
    const onCreate = vi.fn().mockResolvedValue(undefined);
    render(
      <RotationPanel
        profiles={[{ ...profile, status: "active" }]}
        signers={[signerA, signerB]}
        rotations={[]}
        canManage
        busy={false}
        onCreate={onCreate}
        onLoad={async () => {}}
        onActivate={async () => {}}
        onRevoke={async () => {}}
      />,
    );

    fireEvent.change(screen.getByLabelText("Custody profile"), { target: { value: profile.id } });
    fireEvent.change(screen.getByLabelText("Current signer"), { target: { value: signerA.id } });
    fireEvent.change(screen.getByLabelText("Replacement signer"), { target: { value: signerB.id } });
    fireEvent.click(screen.getByRole("button", { name: "Create rotation" }));
    await waitFor(() =>
      expect(onCreate).toHaveBeenCalledWith({
        profile_id: profile.id,
        old_signer_id: signerA.id,
        new_signer_id: signerB.id,
      }),
    );
  });

  it("loads a rotation and confirms revocation separately", async () => {
    const onLoad = vi.fn().mockResolvedValue(undefined);
    const onRevoke = vi.fn().mockResolvedValue(undefined);
    render(
      <RotationPanel
        profiles={[profile]}
        signers={[signerA, signerB]}
        rotations={[rotation]}
        canManage
        busy={false}
        onCreate={async () => {}}
        onLoad={onLoad}
        onActivate={async () => {}}
        onRevoke={onRevoke}
      />,
    );

    fireEvent.change(screen.getByLabelText("Rotation ID"), { target: { value: rotation.id } });
    fireEvent.click(screen.getByRole("button", { name: "Load rotation" }));
    await waitFor(() => expect(onLoad).toHaveBeenCalledWith(rotation.id));

    fireEvent.click(screen.getByRole("button", { name: "Revoke" }));
    expect(onRevoke).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Confirm revoke" }));
    await waitFor(() => expect(onRevoke).toHaveBeenCalledWith(rotation));
  });
});
