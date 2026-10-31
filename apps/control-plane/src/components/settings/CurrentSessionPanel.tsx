"use client";

import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import { auth } from "@/lib/api";
import { getUserSessions, revokeUserSession, type UserSessionView } from "@/lib/api/security-api";
import { clearSession, sessionStore } from "@/lib/auth";

function formatSessionTime(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "Unavailable" : date.toLocaleString();
}

/** Server-authoritative session inventory and per-session revocation controls. */
export default function CurrentSessionPanel() {
  const { session } = useSyncExternalStore(sessionStore.subscribe, sessionStore.getSnapshot, sessionStore.getSnapshot);
  const [sessions, setSessions] = useState<UserSessionView[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pendingId, setPendingId] = useState<string | null>(null);

  const loadSessions = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      const response = await getUserSessions();
      setSessions(response.items);
    } catch (err: unknown) {
      setLoadError(err instanceof Error ? err.message : "Could not load session inventory");
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void Promise.resolve().then(() => loadSessions());
  }, [loadSessions]);

  async function signOutCurrentSession() {
    if (!confirm("Sign out and revoke this session?")) return;
    setPendingId(session?.id ?? "current");
    setActionError(null);
    try {
      await auth.logout();
      clearSession("logout");
    } catch (err: unknown) {
      setActionError(err instanceof Error ? err.message : "Could not revoke this session");
    } finally {
      setPendingId(null);
    }
  }

  async function revoke(item: UserSessionView) {
    if (item.current) {
      await signOutCurrentSession();
      return;
    }
    if (item.state !== "active" || !confirm(`Revoke session ${item.prefix}?`)) return;
    setPendingId(item.id);
    setActionError(null);
    try {
      await revokeUserSession(item.id);
      await loadSessions();
    } catch (err: unknown) {
      setActionError(err instanceof Error ? err.message : "Could not revoke this session");
    } finally {
      setPendingId(null);
    }
  }

  return (
    <section className="card" aria-labelledby="session-inventory-heading">
      <h2 id="session-inventory-heading">Sessions</h2>
      <p className="muted">Review sessions recorded for your account. Revoking a session invalidates its server-side credential.</p>
      {loadError && (
        <div role="alert">
          <p>{loadError}</p>
          <button type="button" onClick={() => void loadSessions()}>Retry session list</button>
        </div>
      )}
      {loading && <p aria-live="polite">Loading sessions…</p>}
      {!loading && !loadError && sessions.length === 0 && <p>No sessions were returned for this account.</p>}
      {!loading && !loadError && sessions.length > 0 && (
        <ul className="stack" aria-label="Session list">
          {sessions.map((item) => (
            <li key={item.id} className="card">
              <p><strong>{item.prefix}</strong>{item.current ? " — Current session" : " — Other session"}</p>
              <p>{item.user_agent || "Browser details unavailable"} · {item.ip || "IP unavailable"}</p>
              <p>Last active: {formatSessionTime(item.last_seen_at)}</p>
              <p>Expires: {formatSessionTime(item.expires_at)}</p>
              <p>Status: {item.state}</p>
              {item.current && item.state === "active" && (
                <button
                  type="button"
                  disabled={pendingId === item.id || pendingId === "current"}
                  onClick={() => void revoke(item)}
                >
                  Sign out this session
                </button>
              )}
              {!item.current && item.state === "active" && (
                <button
                  type="button"
                  aria-label={`Revoke session ${item.prefix}`}
                  disabled={pendingId === item.id}
                  onClick={() => void revoke(item)}
                >
                  {pendingId === item.id ? "Revoking…" : "Revoke session"}
                </button>
              )}
            </li>
          ))}
        </ul>
      )}
      {actionError && <p role="alert">{actionError}</p>}
    </section>
  );
}
