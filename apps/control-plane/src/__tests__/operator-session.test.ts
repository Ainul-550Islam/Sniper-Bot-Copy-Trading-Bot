// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  OPERATOR_KEY_IDLE_TTL_MS,
  clearOperatorKey,
  getOperatorKey,
  getOperatorSessionSnapshot,
  hasOperatorKey,
  operatorKeyHash,
  setOperatorKey,
  subscribeOperatorSession,
} from "../lib/operator-session";
import { clearSession, establishSession } from "../lib/auth";

const KEY = "operator-key-material-0123456789abcdef";

function signIn(platformAdmin = true): void {
  establishSession({
    token: "session-token",
    user: {
      id: "usr-1",
      email: "admin@example.test",
      email_verified: true,
      display_name: "Platform Admin",
      status: "active",
      platform_admin: platformAdmin,
      created_at: new Date().toISOString(),
      last_login_at: null,
    },
    session: {
      id: "ses-1",
      prefix: "ses_1",
      expires_at: new Date(Date.now() + 3_600_000).toISOString(),
      organization_id: "org-1",
    },
  });
}

describe("operator session (in-memory operator API key)", () => {
  let storageSpies: ReturnType<typeof vi.spyOn>[] = [];

  beforeEach(() => {
    signIn();
    // Every storage surface must be untouched by this module. jsdom provides
    // all three, so the spies are real and a single call fails the test.
    storageSpies = [
      vi.spyOn(window.localStorage.__proto__, "getItem"),
      vi.spyOn(window.localStorage.__proto__, "setItem"),
      vi.spyOn(window.localStorage.__proto__, "removeItem"),
      vi.spyOn(window.sessionStorage.__proto__, "getItem"),
      vi.spyOn(window.sessionStorage.__proto__, "setItem"),
      vi.spyOn(window.sessionStorage.__proto__, "removeItem"),
    ];
  });

  afterEach(() => {
    clearOperatorKey();
    clearSession("logout");
    vi.restoreAllMocks();
  });

  it("stores the key in memory and clears it", () => {
    expect(hasOperatorKey()).toBe(false);
    expect(getOperatorKey()).toBeNull();

    setOperatorKey(KEY);

    expect(hasOperatorKey()).toBe(true);
    expect(getOperatorKey()).toBe(KEY);
    expect(getOperatorSessionSnapshot().key).toBe(KEY);

    clearOperatorKey();
    expect(hasOperatorKey()).toBe(false);
    expect(getOperatorKey()).toBeNull();
  });

  it("trims the key and treats an empty value as a lock", () => {
    setOperatorKey(`  ${KEY}  `);
    expect(getOperatorKey()).toBe(KEY);

    setOperatorKey("   ");
    expect(hasOperatorKey()).toBe(false);
  });

  it("notifies subscribers on unlock and on lock", () => {
    const listener = vi.fn();
    const unsubscribe = subscribeOperatorSession(listener);

    setOperatorKey(KEY);
    expect(listener).toHaveBeenCalledTimes(1);
    expect(getOperatorSessionSnapshot().key).toBe(KEY);

    clearOperatorKey();
    expect(listener).toHaveBeenCalledTimes(2);
    expect(getOperatorSessionSnapshot().key).toBeNull();

    // Unsubscribing stops the notifications.
    unsubscribe();
    setOperatorKey(KEY);
    expect(listener).toHaveBeenCalledTimes(2);
  });

  it("keeps a referentially stable snapshot while locked", () => {
    const first = getOperatorSessionSnapshot();
    const second = getOperatorSessionSnapshot();
    expect(first).toBe(second);
  });

  it("locks itself after 15 minutes of idleness", () => {
    vi.useFakeTimers();
    try {
      setOperatorKey(KEY);
      expect(hasOperatorKey()).toBe(true);

      vi.advanceTimersByTime(OPERATOR_KEY_IDLE_TTL_MS - 1);
      expect(hasOperatorKey()).toBe(true);

      vi.advanceTimersByTime(1);
      expect(hasOperatorKey()).toBe(false);
      expect(getOperatorKey()).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("re-arms the idle timer on every unlock", () => {
    vi.useFakeTimers();
    try {
      setOperatorKey(KEY);
      vi.advanceTimersByTime(OPERATOR_KEY_IDLE_TTL_MS - 1);
      // Activity (a fresh unlock) restarts the window rather than inheriting
      // the remainder of the previous one.
      setOperatorKey(KEY);
      vi.advanceTimersByTime(OPERATOR_KEY_IDLE_TTL_MS - 1);
      expect(hasOperatorKey()).toBe(true);
      vi.advanceTimersByTime(1);
      expect(hasOperatorKey()).toBe(false);
    } finally {
      vi.useRealTimers();
    }
  });

  it("records the backend refusal reason only when a 401 cleared the key", () => {
    setOperatorKey(KEY);
    clearOperatorKey("invalid or missing x-api-key");
    expect(getOperatorSessionSnapshot().lastRefusal).toBe(
      "invalid or missing x-api-key",
    );

    // A deliberate lock (button / sign-out / idle expiry) blames nobody.
    setOperatorKey(KEY);
    clearOperatorKey();
    expect(getOperatorSessionSnapshot().lastRefusal).toBeNull();
  });

  it("drops the key when the SaaS session is torn down", () => {
    setOperatorKey(KEY);
    expect(hasOperatorKey()).toBe(true);

    clearSession("logout");
    expect(hasOperatorKey()).toBe(false);
  });

  it("never touches localStorage or sessionStorage", () => {
    setOperatorKey(KEY);
    expect(getOperatorKey()).toBe(KEY);
    clearOperatorKey("refused");
    setOperatorKey(KEY);
    clearSession("logout");

    for (const spy of storageSpies) {
      expect(spy).not.toHaveBeenCalled();
    }
    expect(storageSpies.length).toBe(6);
  });

  it("hashes the in-memory key the same way the backend does", async () => {
    // Rust side: bot_core::auth::sha256_hex = lowercase hex SHA-256.
    const expected = Array.from(
      new Uint8Array(
        await crypto.subtle.digest("SHA-256", new TextEncoder().encode(KEY)),
      ),
    )
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("");

    expect(await operatorKeyHash()).toBeNull();
    setOperatorKey(KEY);
    await expect(operatorKeyHash()).resolves.toBe(expected);
    expect(expected).toMatch(/^[0-9a-f]{64}$/);
  });
});
