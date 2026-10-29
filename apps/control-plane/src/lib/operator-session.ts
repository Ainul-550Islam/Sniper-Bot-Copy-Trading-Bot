"use client";

/**
 * In-memory store for the OPERATOR API KEY (Round 2, defect D1).
 *
 * The operator routes on the backend are not SaaS tenant routes: they are
 * guarded by `require_role`, which authenticates with `extract_key`
 * (`x-api-key` first, then `Authorization: Bearer …`) against the operator key
 * registry via a SHA-256 lookup. A SaaS session token is not an operator
 * credential and is rejected with 401.
 *
 * This module therefore holds a SEPARATE credential from `lib/auth.ts`:
 *
 * - the value lives in a module variable only — never localStorage,
 *   sessionStorage, an IndexedDB, a cookie, a URL or a log line. A page reload
 *   means the console is locked again, which is the intended trade-off for a
 *   key that can halt trading;
 * - it is dropped automatically after 15 minutes of idleness, and immediately
 *   when the SaaS session is torn down (logout/expiry), so signing out of the
 *   control plane can never leave operator authority behind;
 * - it is never echoed back by the UI: only the key the operator types is ever
 *   in memory, and `clear` overwrites nothing because the binding is simply
 *   dropped.
 *
 * The store is tiny and synchronous so it can be read from `useSyncExternalStore`
 * without tearing.
 */

import { sessionStore } from "@/lib/auth";

/** Idle lifetime of an unlocked operator console: 15 minutes. */
export const OPERATOR_KEY_IDLE_TTL_MS = 15 * 60 * 1000;

export interface OperatorSessionSnapshot {
  /** The operator API key, or null while the console is locked. */
  key: string | null;
  /**
   * The backend's refusal reason from the 401 that locked the console, or null
   * when it was locked deliberately (Lock button / sign-out / idle expiry).
   * Surfaced verbatim so "invalid or missing x-api-key" is actionable.
   */
  lastRefusal: string | null;
}

type Listener = () => void;

/** Frozen, shared "locked" snapshot so `getSnapshot` is referentially stable. */
const LOCKED: OperatorSessionSnapshot = Object.freeze({ key: null, lastRefusal: null });

let snapshot: OperatorSessionSnapshot = LOCKED;
const listeners = new Set<Listener>();
let idleTimer: ReturnType<typeof setTimeout> | null = null;

function emit(): void {
  for (const listener of Array.from(listeners)) listener();
}

function cancelIdleExpiry(): void {
  if (idleTimer !== null) {
    clearTimeout(idleTimer);
    idleTimer = null;
  }
}

/** Re-armed on every unlock: an idle console locks itself. */
function scheduleIdleExpiry(): void {
  cancelIdleExpiry();
  idleTimer = setTimeout(() => {
    idleTimer = null;
    clearOperatorKey();
  }, OPERATOR_KEY_IDLE_TTL_MS);
}

/** True when an operator key is currently held in memory. */
export function hasOperatorKey(): boolean {
  return snapshot.key !== null;
}

/** The current key, or null. Never logged, never rendered. */
export function getOperatorKey(): string | null {
  return snapshot.key;
}

/**
 * Unlock the console with an operator API key. An empty/whitespace value locks
 * it instead, so a cleared input can never be mistaken for a credential.
 */
export function setOperatorKey(next: string): void {
  const trimmed = next.trim();
  if (!trimmed) {
    clearOperatorKey();
    return;
  }
  snapshot = Object.freeze({ key: trimmed, lastRefusal: null });
  scheduleIdleExpiry();
  emit();
}

/**
 * Drop the operator key from memory. There is no recovery and no undo.
 *
 * `reason` is the backend's refusal text when the key was cleared because the
 * server answered 401; it is omitted for a deliberate lock (Lock button,
 * sign-out, idle expiry) so the UI never blames the backend for one.
 */
export function clearOperatorKey(reason?: string): void {
  cancelIdleExpiry();
  const lastRefusal = reason?.trim() ? reason.trim() : null;
  if (snapshot.key === null && snapshot.lastRefusal === lastRefusal) {
    return;
  }
  snapshot = lastRefusal === null ? LOCKED : Object.freeze({ key: null, lastRefusal });
  emit();
}

/**
 * SHA-256 (lowercase hex) of the key currently in memory — the same digest the
 * backend computes (`bot_core::auth::sha256_hex`) and the value `/api/keys`
 * reports as `hash`.
 *
 * Used for exactly one thing: recognising the key that is unlocking this
 * console in the key list, so revoking it demands a stronger confirmation than
 * revoking any other key. Returns null when no key is held, or when Web Crypto
 * is unavailable (it is only offered in secure contexts), in which case the UI
 * simply cannot mark the in-use key and every revoke keeps the normal gate.
 *
 * The digest is a lookup key, not a secret: the backend already returns it.
 */
export async function operatorKeyHash(): Promise<string | null> {
  const key = snapshot.key;
  if (key === null) return null;
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) return null;
  const digest = await subtle.digest("SHA-256", new TextEncoder().encode(key));
  let hex = "";
  for (const byte of new Uint8Array(digest)) {
    hex += byte.toString(16).padStart(2, "0");
  }
  return hex;
}

/** Subscribe to lock/unlock transitions. Returns the unsubscribe function. */
export function subscribeOperatorSession(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** `useSyncExternalStore` snapshot: stable while the state is unchanged. */
export function getOperatorSessionSnapshot(): OperatorSessionSnapshot {
  return snapshot;
}

export const operatorSessionStore = {
  subscribe: subscribeOperatorSession,
  getSnapshot: getOperatorSessionSnapshot,
};

// Signing out of (or being expired out of) the control plane must never leave
// operator authority in memory for the next user of the browser tab.
sessionStore.subscribe(() => {
  if (sessionStore.getSnapshot().user === null) clearOperatorKey();
});
