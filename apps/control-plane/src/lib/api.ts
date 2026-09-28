/**
 * The typed control-plane API client (TASK 7B file 06).
 *
 * Rules this file enforces:
 *
 * - Every request is typed; every refusal is an {@link ApiError} carrying
 *   the backend's stable `error` kind and human `reason`.
 * - The session token and the selected tenant id are attached as headers
 *   (`Authorization: Bearer …`, `x-organization: …`) — NEVER as URL query
 *   parameters, which end up in proxy logs and browser history.
 * - There is no database, no secret store and no localStorage here: the
 *   token lives only in memory (see `lib/auth.ts`) and the tenant choice is
 *   a hint the backend re-verifies on every call — the browser is never an
 *   authorization boundary.
 */

/** Base origin of the control-plane API ("same origin" when unset). */
export const API_ORIGIN: string = process.env.NEXT_PUBLIC_API_ORIGIN ?? "";

/** A stable backend refusal: `{ error, reason }` (see SAAS-SECURITY.md). */
export class ApiError extends Error {
  readonly kind: string;
  readonly reason: string;
  readonly status: number;

  constructor(kind: string, reason: string, status: number) {
    super(`${kind}: ${reason}`);
    this.name = "ApiError";
    this.kind = kind;
    this.reason = reason;
    this.status = status;
  }
}

/**
 * Header providers, registered once by `lib/auth.ts` (the single owner of
 * the in-memory session token and tenant choice).
 */
let tokenProvider: () => string | null = () => null;
let tenantProvider: () => string | null = () => null;

export function configureCredentials(
  token: () => string | null,
  tenant: () => string | null,
): void {
  tokenProvider = token;
  tenantProvider = tenant;
}

export type Json = Record<string, unknown>;

/** Options for {@link request}. */
export interface RequestOptions {
  method?: "GET" | "POST" | "PATCH" | "DELETE";
  body?: unknown;
  /** Extra headers for one-off calls. */
  headers?: Record<string, string>;
  /** Skip attaching the session/tenant headers (public endpoints). */
  anonymous?: boolean;
  /** Attach the tenant header even when the client has no selected tenant. */
  signal?: AbortSignal;
}

async function parseError(response: Response): Promise<ApiError> {
  let kind = `http_${response.status}`;
  let reason = response.statusText || "request refused";
  try {
    const body = (await response.json()) as { error?: unknown; reason?: unknown };
    if (typeof body.error === "string") kind = body.error;
    if (typeof body.reason === "string") reason = body.reason;
  } catch {
    // A non-JSON refusal (proxy, HTML error page) keeps the generic kind.
  }
  return new ApiError(kind, reason, response.status);
}

/** One typed API call. JSON in, JSON out, {@link ApiError} on any refusal. */
export async function request<T>(
  path: string,
  options: RequestOptions = {},
): Promise<T> {
  const headers: Record<string, string> = {
    accept: "application/json",
    ...(options.headers ?? {}),
  };
  if (options.body !== undefined) headers["content-type"] = "application/json";
  if (!options.anonymous) {
    const token = tokenProvider();
    if (token) headers["authorization"] = `Bearer ${token}`;
    const tenant = tenantProvider();
    if (tenant) headers["x-organization"] = tenant;
  }
  const response = await fetch(`${API_ORIGIN}${path}`, {
    method: options.method ?? "GET",
    headers,
    body: options.body === undefined ? undefined : JSON.stringify(options.body),
    signal: options.signal,
    // No credentials/cookies: the session travels in the header only.
    credentials: "omit",
    cache: "no-store",
  });
  if (!response.ok) throw await parseError(response);
  if (response.status === 204) return {} as T;
  return (await response.json()) as T;
}

// ---------------------------------------------------------------------------
// Payload types — mirrors of the TASK 7A/7B wire contract (openapi.json is
// the machine-readable source of truth; these are its TypeScript shapes).
// ---------------------------------------------------------------------------

export interface UserProfile {
  id: string;
  email: string;
  email_verified: boolean;
  display_name: string;
  status: string;
  platform_admin: boolean;
  created_at: string;
  last_login_at: string | null;
}

export interface MembershipSummary {
  organization_id: string;
  slug: string;
  name: string;
  status: string;
  role: string;
  membership_status: string;
}

export interface CurrentUserResponse {
  user: UserProfile;
  organizations: MembershipSummary[];
}

export interface LoginResponse {
  user: UserProfile;
  token: string;
  session: {
    id: string;
    prefix: string;
    expires_at: string;
    organization_id: string | null;
  };
}

export interface ApiKeyMetadata {
  id: string;
  key_prefix: string;
  label: string;
  role: string;
  created_at: string;
  expires_at: string | null;
  revoked_at: string | null;
  usable: boolean;
}

export interface ApiKeyCreated {
  key: ApiKeyMetadata;
  /** Shown exactly once, never stored. */
  secret: string;
}

export interface WalletBindingView {
  id: string;
  organization_id: string;
  label: string;
  public_address: string;
  modules: string[];
  created_at: string;
  revoked_at: string | null;
  active: boolean;
}

export interface ExportEnvelope {
  organization_id: string;
  kind: string;
  generated_at: string;
  data: Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// Endpoint groups. Every function maps to a route that EXISTS on the server
// (see docs/SAAS-PRODUCT.md); nothing here invents a URL.
// ---------------------------------------------------------------------------

export const auth = {
  register: (email: string, password: string, displayName?: string) =>
    request<Json>("/api/saas/users", {
      method: "POST",
      body: { email, password, display_name: displayName },
      anonymous: true,
    }),
  login: (email: string, password: string) =>
    request<LoginResponse>("/api/saas/sessions", {
      method: "POST",
      body: { email, password },
      anonymous: true,
    }),
  logout: () => request<Json>("/api/saas/users/me/logout", { method: "POST" }),
  me: () => request<CurrentUserResponse>("/api/saas/users/me"),
  updateProfile: (displayName: string) =>
    request<UserProfile>("/api/saas/users/me", {
      method: "PATCH",
      body: { display_name: displayName },
    }),
};

export const tenants = {
  createOrganization: (slug: string, name: string) =>
    request<Json>("/api/saas/organizations", {
      method: "POST",
      body: { slug, name },
    }),
  organization: (id: string) => request<Json>(`/api/saas/organizations/${id}`),
  members: (id: string) =>
    request<Json>(`/api/saas/organizations/${id}/members`),
};

export const apiKeys = {
  create: (label: string, role?: string) =>
    request<ApiKeyCreated>("/api/saas/api-keys", {
      method: "POST",
      body: { label, ...(role ? { role } : {}) },
    }),
  list: () => request<{ keys?: ApiKeyMetadata[] }>("/api/saas/api-keys"),
  revoke: (prefix: string) =>
    request<Json>(`/api/saas/api-keys/${prefix}`, { method: "DELETE" }),
};

export const wallets = {
  list: () => request<{ organization_id: string; wallets: WalletBindingView[] }>("/api/saas/wallet-access"),
  bind: (label: string, publicAddress: string, modules: string[]) =>
    request<WalletBindingView>("/api/saas/wallet-access", {
      method: "POST",
      body: { label, public_address: publicAddress, modules },
    }),
  revoke: (id: string) =>
    request<Json>(`/api/saas/wallet-access/${id}`, { method: "DELETE" }),
};

export const exportsApi = {
  get: (kind: string) => request<ExportEnvelope>(`/api/saas/exports?kind=${encodeURIComponent(kind)}`),
};

/** Sections whose truth lives in the operator console (TASK 1–6 engines). */
export const operator = {
  status: () => request<Json>("/api/status"),
  orders: () => request<Json>("/api/orders"),
  positions: () => request<Json>("/api/positions"),
  risk: () => request<Json>("/api/risk/global"),
  portfolio: () => request<Json>("/api/accounting/portfolio"),
  findings: () => request<Json>("/api/reconciliation/findings"),
  ha: () => request<Json>("/api/ha"),
};
