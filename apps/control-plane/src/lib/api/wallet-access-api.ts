/**
 * Wallet access authorization API client (Part 04, L5).
 *
 * Covers `POST /api/saas/wallet-access/{id}/authorize` — authorizing a
 * tenant wallet binding (public address only, no key material) for a
 * trading module. Authorization is destructive-equivalent: the page confirms
 * before the POST is sent.
 */

import { request } from "../api";
import type { WalletBindingView } from "../api";

export interface WalletAccessAuthorizeInput {
  module: "module.sniper" | "module.copy" | "module.polymarket";
}

export interface WalletAccessAuthorizeResponse {
  allowed: boolean;
  binding: WalletBindingView;
  reason: string;
}

export const walletAccessApi = {
  authorize: (id: string, input: WalletAccessAuthorizeInput) =>
    request<WalletAccessAuthorizeResponse>(
      `/api/saas/wallet-access/${encodeURIComponent(id)}/authorize`,
      { method: "POST", body: input },
    ),
};
