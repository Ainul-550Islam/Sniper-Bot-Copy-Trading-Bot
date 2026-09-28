"use client";

/**
 * Session state, wired to the TASK 7A session model (TASK 7B file 07).
 *
 * The model is the server's: a session is created by `POST /api/saas/sessions`,
 * carries a 12-hour expiry, is identified by a one-time plaintext token, and
 * dies on logout, expiry or revocation. This module:
 *
 * - keeps the plaintext token IN MEMORY ONLY (a module variable) — never in
 *   localStorage, sessionStorage, cookies or any URL. Reloading the page
 *   therefore requires signing in again; that is the price of never
 *   persisting a credential in a browser store, and it is a deliberate
 *   trade-off for a control plane that can move money.
 * - does NO hashing, NO "validation" of the token's permission beyond
 *   reading expiry (all authorization happens server-side);
 * - exposes the selected tenant as a plain client-side hint — the backend
 *   re-resolves and re-verifies the tenant on every request
 *   (`x-organization` can only CONFIRM, never switch).
 */

import { ApiError, configureCredentials, type CurrentUserResponse, type MembershipSummary } from "@/lib/api";

/** Server-side session facts (no secret material). */
export interface SessionInfo {
  id: string;
  prefix: string;
  expiresAt: number;
  organizationId: string | null;
}

export interface AuthState {
  /** The user profile, when a session is established. */
  user: CurrentUserResponse["user"] | null;
  /** The organizations the user belongs to — the ONLY tenant choices. */
  organizations: MembershipSummary[];
  /** Server-side session facts, when established. */
  session: SessionInfo | null;
  /** The client-side tenant hint (an id from {@link AuthState.organizations}). */
  selectedOrganizationId: string | null;
}

type Listener = () => void;

let token: string | null = null;
let state: AuthState = {
  user: null,
  organizations: [],
  session: null,
  selectedOrganizationId: null,
};

const listeners = new Set<Listener>();

function setState(patch: Partial<AuthState>): void {
  state = { ...state, ...patch };
  listeners.forEach((l) => l());
}

function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function getSnapshot(): AuthState {
  return state;
}

// The API client reads the credential and tenant hint from HERE — the single
// source of truth on the client.
configureCredentials(
  () => token,
  () => state.selectedOrganizationId,
);

/** Milliseconds until the session expires (0 when already expired/absent). */
export function msUntilExpiry(): number {
  const session = state.session;
  if (!session) return 0;
  return Math.max(0, session.expiresAt - Date.now());
}

/** True when a session exists AND its 12-hour window has not passed. */
export function hasLiveSession(): boolean {
  return token !== null && msUntilExpiry() > 0;
}

/** The selected tenant hint, or null. Only ids from `organizations` qualify. */
export function selectOrganization(organizationId: string | null): void {
  const known =
    organizationId === null ||
    state.organizations.some((m) => m.organization_id === organizationId);
  if (!known) {
    // A browser-supplied organization id can never widen access; the server
    // would refuse it anyway. The UI simply refuses to select it.
    return;
  }
  setState({ selectedOrganizationId: organizationId });
}

/** Establish a session. The token is held in memory only. */
export async function login(email: string, password: string): Promise<void> {
  const { auth } = await import("@/lib/api");
  const response = await auth.login(email, password);
  token = response.token;
  const expiresAt = Date.parse(response.session.expires_at);
  setState({
    user: response.user,
    organizations: [],
    session: {
      id: response.session.id,
      prefix: response.session.prefix,
      expiresAt: Number.isNaN(expiresAt) ? 0 : expiresAt,
      organizationId: response.session.organization_id,
    },
    selectedOrganizationId: null,
  });
  // Immediately hydrate the membership list so the tenant switcher shows the
  // user's OWN organizations (never free-typed ids).
  await refresh();
  // Pre-select the tenant the login response named, when it is one of ours.
  const loginOrg = response.session.organization_id;
  if (loginOrg && state.organizations.some((m) => m.organization_id === loginOrg)) {
    selectOrganization(loginOrg);
  } else if (state.organizations.length === 1) {
    selectOrganization(state.organizations[0]!.organization_id);
  }
}

/** Create an account, then sign in with the same credentials. */
export async function register(
  email: string,
  password: string,
  displayName: string,
): Promise<void> {
  const { auth } = await import("@/lib/api");
  await auth.register(email, password, displayName);
  await login(email, password);
}

/**
 * Re-read the session's truth from the server: user profile + membership
 * list. Doubts about the session are settled by the server's answer, never
 * by client-side logic. Returns false when the session is gone.
 */
export async function refresh(): Promise<boolean> {
  if (token === null) return false;
  try {
    const { auth } = await import("@/lib/api");
    const me = await auth.me();
    setState({
      user: me.user,
      organizations: me.organizations,
    });
    // The selected tenant must still be one of the CURRENT memberships.
    const selected = state.selectedOrganizationId;
    if (selected && !me.organizations.some((m) => m.organization_id === selected)) {
      setState({ selectedOrganizationId: null });
    }
    return true;
  } catch (error) {
    if (error instanceof ApiError && (error.status === 401 || error.status === 403)) {
      clearSession("expired");
      return false;
    }
    throw error;
  }
}

/** Server-side logout (revoke) plus local teardown. */
export async function logout(): Promise<void> {
  try {
    const { auth } = await import("@/lib/api");
    await auth.logout();
  } catch {
    // The server may already have revoked it; local teardown happens either way.
  } finally {
    clearSession("logout");
  }
}

/** Drop the credential from memory. The token is gone — no recovery. */
export function clearSession(reason: "logout" | "expired"): void {
  void reason; // callers may surface this; the token itself is simply dropped
  token = null;
  setState({
    user: null,
    organizations: [],
    session: null,
    selectedOrganizationId: null,
  });
}

export const sessionStore = { subscribe, getSnapshot };
