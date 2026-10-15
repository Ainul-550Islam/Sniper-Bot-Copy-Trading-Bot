"use client";

import { useCallback, useEffect, useState } from "react";

import AppShell from "@/components/AppShell";
import { ErrorState } from "@/components/common/ErrorState";
import { toDisplayError } from "@/lib/api";
import {
  deleteSsoConfig,
  getSsoConfig,
  putSsoConfig,
  ssoAuthorizeUrl,
  type SsoConfig,
  type SsoProviderType,
} from "@/lib/api/sso-api";

const PROVIDERS: Array<{ value: SsoProviderType; label: string }> = [
  { value: "oidc", label: "Generic OIDC" },
  { value: "google", label: "Google" },
  { value: "okta", label: "Okta" },
  { value: "azure_ad", label: "Microsoft Entra ID (Azure AD)" },
];

/** Callback URL the IdP must be given. Derived from the page origin, never typed. */
function defaultRedirectUri(): string {
  if (typeof window === "undefined") return "";
  return `${window.location.origin}/api/saas/sso/callback`;
}

export default function SsoSettingsPage() {
  const [config, setConfig] = useState<SsoConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const [providerType, setProviderType] = useState<SsoProviderType>("oidc");
  const [issuerUrl, setIssuerUrl] = useState("");
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const [domains, setDomains] = useState("");
  const [enforce, setEnforce] = useState(false);

  const applyConfig = useCallback((value: SsoConfig | null) => {
    setConfig(value);
    setProviderType((value?.provider_type as SsoProviderType) ?? "oidc");
    setIssuerUrl(value?.issuer_url ?? "");
    setClientId(value?.client_id ?? "");
    setClientSecret("");
    setDomains((value?.allowed_domains ?? []).join(", "));
    setEnforce(value?.enforce_sso ?? false);
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      applyConfig(await getSsoConfig());
    } catch (err: unknown) {
      setError(toDisplayError(err));
    } finally {
      setLoading(false);
    }
  }, [applyConfig]);

  useEffect(() => {
    void Promise.resolve().then(() => load());
  }, [load]);

  async function save(event: React.FormEvent) {
    event.preventDefault();
    setSaving(true);
    setError(null);
    setNotice(null);
    try {
      const saved = await putSsoConfig({
        provider_type: providerType,
        issuer_url: issuerUrl.trim(),
        client_id: clientId.trim(),
        ...(clientSecret ? { client_secret: clientSecret } : {}),
        redirect_uri: config?.redirect_uri || defaultRedirectUri(),
        allowed_domains: domains
          .split(",")
          .map((d) => d.trim().toLowerCase())
          .filter(Boolean),
        role_mapping: config?.role_mapping ?? {},
        enforce_sso: enforce,
      });
      applyConfig(saved);
      setNotice("SSO configuration saved.");
    } catch (err: unknown) {
      setError(toDisplayError(err));
    } finally {
      setSaving(false);
    }
  }

  async function remove() {
    if (!window.confirm("Remove SSO for this organization? Members will sign in with passwords again.")) return;
    setError(null);
    try {
      await deleteSsoConfig();
      setNotice("SSO configuration removed.");
      await load();
    } catch (err: unknown) {
      setError(toDisplayError(err));
    }
  }

  const redirect = config?.redirect_uri || defaultRedirectUri();

  return (
    <AppShell title="Single sign-on">
      <div style={{ marginBottom: "1.5rem" }}>
        <h1 style={{ margin: 0 }}>Single sign-on (SSO)</h1>
        <p style={{ margin: "0.25rem 0 0", color: "var(--muted)", fontSize: "0.9rem" }}>
          Connect your identity provider. Requires the SecurityAdmin role.
        </p>
      </div>

      {error ? <ErrorState title="SSO settings unavailable" error={error} onRetry={() => void load()} /> : null}
      {notice ? (
        <div role="status" className="card" style={{ marginBottom: "1rem" }}>
          {notice}
        </div>
      ) : null}

      {loading ? (
        <div className="card">Loading SSO settings…</div>
      ) : (
        <form className="card" onSubmit={save} style={{ display: "grid", gap: "0.9rem", maxWidth: 640 }}>
          <label>
            Provider
            <select value={providerType} onChange={(e) => setProviderType(e.target.value as SsoProviderType)} style={{ display: "block", width: "100%" }}>
              {PROVIDERS.map((p) => (
                <option key={p.value} value={p.value}>
                  {p.label}
                </option>
              ))}
            </select>
          </label>

          <label>
            Issuer URL
            <input value={issuerUrl} onChange={(e) => setIssuerUrl(e.target.value)} placeholder="https://idp.example.com" required style={{ display: "block", width: "100%" }} />
          </label>

          <label>
            Client ID
            <input value={clientId} onChange={(e) => setClientId(e.target.value)} required style={{ display: "block", width: "100%" }} />
          </label>

          <label>
            Client secret {config?.client_secret_set ? "(set — leave blank to keep)" : "(optional for PKCE-only clients)"}
            <input
              type="password"
              autoComplete="new-password"
              value={clientSecret}
              onChange={(e) => setClientSecret(e.target.value)}
              placeholder={config?.client_secret_set ? "••••••••" : ""}
              style={{ display: "block", width: "100%" }}
            />
          </label>

          <label>
            Redirect URI (register this with your IdP)
            <input value={redirect} readOnly style={{ display: "block", width: "100%", color: "var(--muted)" }} />
          </label>

          <label>
            Allowed email domains (comma separated; empty = any)
            <input value={domains} onChange={(e) => setDomains(e.target.value)} placeholder="example.com, corp.example.com" style={{ display: "block", width: "100%" }} />
          </label>

          <label style={{ display: "flex", gap: "0.5rem", alignItems: "center" }}>
            <input type="checkbox" checked={enforce} onChange={(e) => setEnforce(e.target.checked)} />
            Require SSO for all members (password sign-in is blocked when enforced)
          </label>

          <div style={{ display: "flex", gap: "0.5rem", flexWrap: "wrap" }}>
            <button type="submit" className="btn btn-primary" disabled={saving}>
              {saving ? "Saving…" : config ? "Save changes" : "Enable SSO"}
            </button>
            {config ? (
              <a className="btn btn-secondary" href={ssoAuthorizeUrl(config.organization_id)} target="_blank" rel="noopener noreferrer">
                Test connection (opens IdP sign-in)
              </a>
            ) : null}
            {config ? (
              <button type="button" className="btn btn-secondary" onClick={() => void remove()} style={{ color: "var(--bad)" }}>
                Remove SSO
              </button>
            ) : null}
          </div>
        </form>
      )}
    </AppShell>
  );
}
