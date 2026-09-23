"use client";

import { useCallback, useEffect, useMemo, useState } from "react";

import { TenantGate, TenantSwitcher } from "@/components/TenantSwitcher";
import {
  ApiError,
  apiKeys,
  type ApiKeyCreated,
  type ApiKeyMetadata,
  type Json,
  operator,
  exportsApi,
  auth,
  wallets,
  tenants,
} from "@/lib/api";
import {
  clearSession,
  hasLiveSession,
  logout,
  msUntilExpiry,
  refresh,
  selectOrganization,
  sessionStore,
} from "@/lib/auth";

/**
 * The application shell (TASK 7B file 08).
 *
 * Fourteen sections, in product order: Dashboard, Bots, Orders, Positions,
 * Risk, Accounting, Reconciliation, Workers, Wallets, Team, Audit, Billing,
 * Usage, Settings. Every section reads data the backend actually serves —
 * SaaS sections through `/api/saas/*`, trading-truth sections through the
 * operator console's existing endpoints (which tenant sessions are NOT
 * entitled to; those render an explanatory notice instead of pretending).
 * No route is invented: URLs here are exactly the server's.
 */

type SectionId =
  | "dashboard"
  | "bots"
  | "orders"
  | "positions"
  | "risk"
  | "accounting"
  | "reconciliation"
  | "workers"
  | "wallets"
  | "team"
  | "audit"
  | "billing"
  | "usage"
  | "settings";

const NAV: ReadonlyArray<{ id: SectionId; label: string; group: string }> = [
  { id: "dashboard", label: "Dashboard", group: "Overview" },
  { id: "bots", label: "Bots", group: "Trading (operator console)" },
  { id: "orders", label: "Orders", group: "Trading (operator console)" },
  { id: "positions", label: "Positions", group: "Trading (operator console)" },
  { id: "risk", label: "Risk", group: "Trading (operator console)" },
  { id: "accounting", label: "Accounting", group: "Trading (operator console)" },
  { id: "reconciliation", label: "Reconciliation", group: "Trading (operator console)" },
  { id: "workers", label: "Workers", group: "Trading (operator console)" },
  { id: "wallets", label: "Wallets", group: "Tenant" },
  { id: "team", label: "Team", group: "Tenant" },
  { id: "audit", label: "Audit", group: "Tenant" },
  { id: "billing", label: "Billing", group: "Tenant" },
  { id: "usage", label: "Usage", group: "Tenant" },
  { id: "settings", label: "Settings", group: "Tenant" },
];

const OPERATOR_SECTIONS: ReadonlyArray<SectionId> = [
  "bots",
  "orders",
  "positions",
  "risk",
  "accounting",
  "reconciliation",
  "workers",
];

function currentPeriod(): string {
  return new Date().toISOString().slice(0, 7);
}

/** Pretty, ordered JSON view — the honest rendering of API truth. */
function JsonView({ value }: { value: unknown }) {
  return <pre className="json">{JSON.stringify(value, null, 2)}</pre>;
}

/** Live countdown of the in-memory session's remaining life. */
function SessionBadge() {
  const [remaining, setRemaining] = useState(msUntilExpiry());
  useEffect(() => {
    const timer = setInterval(() => setRemaining(msUntilExpiry()), 1000);
    return () => clearInterval(timer);
  }, []);
  const minutes = Math.floor(remaining / 60000);
  const seconds = Math.floor((remaining % 60000) / 1000);
  return (
    <span className="badge" title="Time until the session expires">
      session {minutes}:{String(seconds).padStart(2, "0")}
    </span>
  );
}

type NavItem = (typeof NAV)[number];

function navGroupsOf(): Array<[string, NavItem[]]> {
  const groups = new Map<string, NavItem[]>();
  for (const item of NAV) {
    const existing = groups.get(item.group) ?? [];
    existing.push(item);
    groups.set(item.group, existing);
  }
  return [...groups.entries()];
}

export function AppShell() {
  const [active, setActive] = useState<SectionId>("dashboard");
  const navGroups = useMemo(navGroupsOf, []);
  const state = sessionStore.getSnapshot();
  const [, forceRender] = useState(0);
  const [expired, setExpired] = useState(false);

  useEffect(() => sessionStore.subscribe(() => forceRender((n) => n + 1)), []);

  // Session expiry watch: when the 12-hour window passes, the session is
  // gone (the server refuses it anyway) — tear down locally and say why.
  useEffect(() => {
    const timer = setInterval(() => {
      if (hasLiveSession() === false) {
        setExpired(true);
        clearSession("expired");
      }
    }, 5000);
    return () => clearInterval(timer);
  }, []);

  if (expired || !hasLiveSession()) {
    return (
      <main id="main" className="landing">
        <div className="card">
          <h1>Session ended</h1>
          <p className="muted">
            Your session expired or was revoked. Reload to sign in again.
          </p>
          <button className="primary" type="button" onClick={() => window.location.reload()}>
            Back to sign-in
          </button>
        </div>
      </main>
    );
  }

  const user = state.user;
  if (!user) return null;

  return (
    <div className="shell">
      <header className="shell__header">
        <div className="shell__brand">
          {/* eslint-disable-next-line @next/next/no-img-element */}
          <img src="/logo.svg" alt="" width={28} height={28} />
          <span>Sniper Suite — Control Plane</span>
        </div>
        <TenantSwitcher
          organizations={state.organizations}
          selected={state.selectedOrganizationId}
          onSelect={selectOrganization}
        />
        <div className="shell__session">
          <SessionBadge />
          <button type="button" onClick={() => void logout()}>
            Sign out
          </button>
        </div>
      </header>

      <div className="shell__body">
        <nav aria-label="Sections" className="shell__nav">
          {navGroups.map(([group, items]) => (
            <div key={group}>
              <h3>{group}</h3>
              <ul>
                {items.map((item) => (
                  <li key={item.id}>
                    <button
                      type="button"
                      aria-current={active === item.id ? "page" : undefined}
                      onClick={() => setActive(item.id)}
                    >
                      {item.label}
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </nav>

        <main id="main" className="shell__main">
          <Section section={active} user={user} />
        </main>
      </div>
    </div>
  );
}

function Section({
  section,
  user,
}: {
  section: SectionId;
  user: NonNullable<ReturnType<typeof sessionStore.getSnapshot>["user"]>;
}) {
  switch (section) {
    case "dashboard":
      return <DashboardSection />;
    case "wallets":
      return <WalletsSection />;
    case "team":
      return <TeamSection />;
    case "audit":
      return <AuditSection />;
    case "billing":
      return <BillingSection />;
    case "usage":
      return <UsageSection />;
    case "settings":
      return <SettingsSection user={user} />;
    default:
      // Bots/Orders/Positions/Risk/Accounting/Reconciliation/Workers —
      // operator-console truth, read-only, via the existing endpoints.
      return <OperatorSection section={section} />;
  }
}

/** Fetch-and-render for one existing operator-console endpoint. */
function OperatorSection({ section }: { section: SectionId }) {
  const paths: Partial<Record<SectionId, () => Promise<Json>>> = {
    bots: operator.status,
    orders: operator.orders,
    positions: operator.positions,
    risk: operator.risk,
    accounting: operator.portfolio,
    reconciliation: operator.findings,
    workers: operator.ha,
  };
  const fetcher = paths[section];
  const [data, setData] = useState<Json | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    setError(null);
    fetcher?.()
      .then((d) => alive && (setData(d), setLoading(false)))
      .catch((e: unknown) => {
        if (!alive) return;
        if (e instanceof ApiError && (e.status === 401 || e.status === 403)) {
          setError(
            "This is operator-console truth (TASK 1–6 engines). It is served to the deployment credential, not to tenant sessions — use the operator console for live trading data.",
          );
        } else {
          setError(e instanceof Error ? e.message : "request failed");
        }
        setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [fetcher, section]);

  const label = NAV.find((n) => n.id === section)?.label ?? section;
  return (
    <article>
      <h2>{label}</h2>
      {loading && <p className="muted">Loading…</p>}
      {error && (
        <p className="notice" role="note">
          {error}
        </p>
      )}
      {data && <JsonView value={data} />}
    </article>
  );
}

function DashboardSection() {
  const state = sessionStore.getSnapshot();
  const user = state.user;
  return (
    <article>
      <h2>Dashboard</h2>
      {user && (
        <div className="cards">
          <div className="card">
            <h3>Signed in</h3>
            <p>
              <strong>{user.display_name}</strong> &lt;{user.email}&gt;
            </p>
            <p className="muted small">
              status {user.status}
              {user.platform_admin ? " · platform admin" : ""}
            </p>
          </div>
          <div className="card">
            <h3>Your organizations</h3>
            <ul>
              {state.organizations.map((m) => (
                <li key={m.organization_id}>
                  {m.name} — <span className={`tag tag--${m.status}`}>{m.status}</span>{" "}
                  <span className="muted small">as {m.role}</span>
                </li>
              ))}
            </ul>
          </div>
          <div className="card">
            <h3>Where is the trading data?</h3>
            <p className="muted small">
              Trading truth (orders, fills, positions, risk, ledger, HA) is
              owned by the operator engines. Tenant sessions manage access —
              wallets, team, billing, usage and audit — here in the SaaS
              control plane.
            </p>
          </div>
        </div>
      )}
    </article>
  );
}

function WalletsSection() {
  const state = sessionStore.getSnapshot();
  const org = state.selectedOrganizationId;
  const [data, setData] = useState<Awaited<ReturnType<typeof wallets.list>> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [label, setLabel] = useState("");
  const [address, setAddress] = useState("");
  const [modules, setModules] = useState<string[]>(["module.sniper"]);

  const load = useCallback(() => {
    if (!org) return;
    setBusy(true);
    wallets
      .list()
      .then(setData)
      .catch((e: unknown) => setError(e instanceof Error ? e.message : "failed"))
      .finally(() => setBusy(false));
  }, [org]);

  useEffect(() => {
    load();
  }, [load]);

  async function bind(event: React.FormEvent) {
    event.preventDefault();
    setError(null);
    setBusy(true);
    try {
      await wallets.bind(label, address, modules);
      setLabel("");
      setAddress("");
      load();
    } catch (e) {
      setError(e instanceof Error ? e.message : "bind failed");
    } finally {
      setBusy(false);
    }
  }

  return (
    <article>
      <h2>Wallets</h2>
      <TenantGate selected={org}>
        <p className="muted small">
          Bindings are public data only (label + public address + modules).
          Signing keys never reach this control plane.
        </p>
        <form className="form card" onSubmit={bind}>
          <label>
            Label
            <input required maxLength={120} value={label} onChange={(e) => setLabel(e.target.value)} />
          </label>
          <label>
            Public address
            <input
              required
              minLength={8}
              maxLength={100}
              pattern="[A-Za-z0-9:_-]+"
              value={address}
              onChange={(e) => setAddress(e.target.value)}
            />
          </label>
          <fieldset>
            <legend>Modules</legend>
            {["module.sniper", "module.copy", "module.polymarket"].map((m) => (
              <label key={m} className="inline">
                <input
                  type="checkbox"
                  checked={modules.includes(m)}
                  onChange={(e) =>
                    setModules((prev) =>
                      e.target.checked ? [...prev, m] : prev.filter((x) => x !== m),
                    )
                  }
                />
                {m}
              </label>
            ))}
          </fieldset>
          <button className="primary" disabled={busy} type="submit">
            Bind wallet
          </button>
        </form>
        {error && <p className="error" role="alert">{error}</p>}
        {data && <JsonView value={data} />}
      </TenantGate>
    </article>
  );
}

function TeamSection() {
  const state = sessionStore.getSnapshot();
  const org = state.selectedOrganizationId;
  const [data, setData] = useState<Json | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!org) return;
    tenants
      .members(org)
      .then((m) => setData(m as unknown as Json))
      .catch((e: unknown) =>
        setError(e instanceof ApiError ? e.reason : e instanceof Error ? e.message : "failed"),
      );
  }, [org]);

  return (
    <article>
      <h2>Team</h2>
      <TenantGate selected={org}>
        {error && <p className="error" role="alert">{error}</p>}
        {data ? <JsonView value={data} /> : <p className="muted">Loading…</p>}
      </TenantGate>
    </article>
  );
}

function AuditSection() {
  const state = sessionStore.getSnapshot();
  const org = state.selectedOrganizationId;
  const [data, setData] = useState<Awaited<ReturnType<typeof exportsApi.get>> | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!org) return;
    exportsApi
      .get("audit")
      .then(setData)
      .catch((e: unknown) =>
        setError(e instanceof ApiError ? `${e.kind}: ${e.reason}` : e instanceof Error ? e.message : "failed"),
      );
  }, [org]);

  return (
    <article>
      <h2>Audit</h2>
      <TenantGate selected={org}>
        <p className="muted small">
          The audit export carries records that target your organization.
        </p>
        {error && <p className="error" role="alert">{error}</p>}
        {data ? <JsonView value={data} /> : <p className="muted">Loading…</p>}
      </TenantGate>
    </article>
  );
}

function BillingSection() {
  const state = sessionStore.getSnapshot();
  const org = state.selectedOrganizationId;
  const [subscription, setSubscription] = useState<Awaited<
    ReturnType<typeof exportsApi.get>
  > | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!org) return;
    exportsApi
      .get("subscription")
      .then(setSubscription)
      .catch((e: unknown) =>
        setError(e instanceof ApiError ? `${e.kind}: ${e.reason}` : e instanceof Error ? e.message : "failed"),
      );
  }, [org]);

  return (
    <article>
      <h2>Billing</h2>
      <TenantGate selected={org}>
        <p className="muted small">
          Your subscription and plan limits, from the deterministic{" "}
          <code>subscription</code> export.
        </p>
        {error && <p className="error" role="alert">{error}</p>}
        {subscription ? <JsonView value={subscription} /> : <p className="muted">Loading…</p>}
      </TenantGate>
    </article>
  );
}

function UsageSection() {
  const state = sessionStore.getSnapshot();
  const org = state.selectedOrganizationId;
  const [data, setData] = useState<Awaited<ReturnType<typeof exportsApi.get>> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const period = useMemo(currentPeriod, []);

  useEffect(() => {
    if (!org) return;
    exportsApi
      .get("usage")
      .then(setData)
      .catch((e: unknown) =>
        setError(e instanceof ApiError ? `${e.kind}: ${e.reason}` : e instanceof Error ? e.message : "failed"),
      );
  }, [org]);

  return (
    <article>
      <h2>Usage — {period}</h2>
      <TenantGate selected={org}>
        <p className="muted small">
          Metered totals for the current period, from the deterministic{" "}
          <code>usage</code> export.
        </p>
        {error && <p className="error" role="alert">{error}</p>}
        {data ? <JsonView value={data} /> : <p className="muted">Loading…</p>}
      </TenantGate>
    </article>
  );
}

function SettingsSection({
  user,
}: {
  user: NonNullable<ReturnType<typeof sessionStore.getSnapshot>["user"]>;
}) {
  const state = sessionStore.getSnapshot();
  const org = state.selectedOrganizationId;
  const [displayName, setDisplayName] = useState(user.display_name);
  const [saved, setSaved] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const [slug, setSlug] = useState("");
  const [orgName, setOrgName] = useState("");

  const [keys, setKeys] = useState<ApiKeyMetadata[] | null>(null);
  const [keyLabel, setKeyLabel] = useState("");
  const [freshSecret, setFreshSecret] = useState<ApiKeyCreated | null>(null);

  const loadKeys = useCallback(() => {
    if (!org) return;
    apiKeys
      .list()
      .then((r) => setKeys(r.keys ?? []))
      .catch(() => setKeys(null));
  }, [org]);
  useEffect(loadKeys, [loadKeys]);

  async function saveProfile(event: React.FormEvent) {
    event.preventDefault();
    setError(null);
    try {
      await auth.updateProfile(displayName);
      await refresh();
      setSaved("Profile updated.");
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed");
    }
  }

  async function createOrg(event: React.FormEvent) {
    event.preventDefault();
    setError(null);
    try {
      await tenants.createOrganization(slug, orgName);
      await refresh();
      setSaved(`Organization ${orgName} created.`);
      setSlug("");
      setOrgName("");
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed");
    }
  }

  async function createKey(event: React.FormEvent) {
    event.preventDefault();
    setError(null);
    setFreshSecret(null);
    try {
      const created = await apiKeys.create(keyLabel);
      setFreshSecret(created);
      setKeyLabel("");
      loadKeys();
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed");
    }
  }

  async function revokeKey(prefix: string) {
    setError(null);
    try {
      await apiKeys.revoke(prefix);
      loadKeys();
    } catch (e) {
      setError(e instanceof Error ? e.message : "revoke failed");
    }
  }

  return (
    <article>
      <h2>Settings</h2>
      {saved && <p className="notice" role="status">{saved}</p>}
      {error && <p className="error" role="alert">{error}</p>}

      <div className="cards">
        <form className="card form" onSubmit={saveProfile}>
          <h3>Profile</h3>
          <label>
            Display name
            <input maxLength={120} value={displayName} onChange={(e) => setDisplayName(e.target.value)} />
          </label>
          <button className="primary" type="submit">Save</button>
        </form>

        <form className="card form" onSubmit={createOrg}>
          <h3>Create organization</h3>
          <label>
            Slug
            <input
              required
              pattern="[a-z0-9][a-z0-9-]{1,62}"
              value={slug}
              onChange={(e) => setSlug(e.target.value)}
            />
          </label>
          <label>
            Name
            <input required maxLength={120} value={orgName} onChange={(e) => setOrgName(e.target.value)} />
          </label>
          <button className="primary" type="submit">Create</button>
        </form>
      </div>

      <TenantGate selected={org}>
        <div className="cards">
          <form className="card form" onSubmit={createKey}>
            <h3>API keys</h3>
            <label>
              Label
              <input required maxLength={120} value={keyLabel} onChange={(e) => setKeyLabel(e.target.value)} />
            </label>
            <button className="primary" type="submit">Create key</button>
            {freshSecret && (
              <div className="notice" role="alert">
                <p>
                  <strong>Copy the secret now — it is shown exactly once.</strong>
                </p>
                <code>{freshSecret.secret}</code>
              </div>
            )}
            {keys && (
              <ul>
                {keys.map((k) => (
                  <li key={k.id}>
                    <code>{k.key_prefix}…</code> {k.label}{" "}
                    <span className={`tag ${k.usable ? "tag--active" : "tag--closed"}`}>
                      {k.usable ? "usable" : "revoked/expired"}
                    </span>{" "}
                    {k.usable && (
                      <button type="button" onClick={() => void revokeKey(k.key_prefix)}>
                        revoke
                      </button>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </form>

          <ExportsCard org={org} onError={setError} />
        </div>
      </TenantGate>
    </article>
  );
}

/** One-click deterministic exports of the tenant's own records. */
function ExportsCard({
  org,
  onError,
}: {
  org: string | null;
  onError: (message: string) => void;
}) {
  const [exported, setExported] = useState<Awaited<ReturnType<typeof exportsApi.get>> | null>(null);
  const kinds = [
    "profile",
    "members",
    "api_keys",
    "usage",
    "subscription",
    "wallets",
    "audit",
  ] as const;

  async function run(kind: (typeof kinds)[number]) {
    try {
      setExported(await exportsApi.get(kind));
    } catch (e) {
      onError(e instanceof ApiError ? `${e.kind}: ${e.reason}` : e instanceof Error ? e.message : "failed");
    }
  }

  return (
    <div className="card form">
      <h3>Your data</h3>
      <p className="muted small">Deterministic exports of your own records.</p>
      {kinds.map((kind) => (
        <button key={kind} type="button" disabled={!org} onClick={() => void run(kind)}>
          export {kind}
        </button>
      ))}
      {exported && <JsonView value={exported} />}
    </div>
  );
}
