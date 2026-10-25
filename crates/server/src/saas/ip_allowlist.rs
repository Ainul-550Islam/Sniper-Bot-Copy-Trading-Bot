//! Tenant IP-allowlist enforcement (ROUND-3 audit fix).
//!
//! # The gap this closes
//!
//! `security.rs` persists `tenant_security_policies.ip_allowlist` and
//! `get_status` reports it back to the dashboard — but nothing read the
//! value. A tenant that configured an allowlist believed its control-plane
//! access was restricted to those CIDRs while every origin was still
//! admitted. This module performs the check, and
//! [`super::middleware::authorize_request_mode`] runs it for every
//! credential branch (tenant API key, user session, legacy deployment key)
//! once the tenant has been resolved, so the rule applies uniformly.
//!
//! # Failure semantics: fail closed
//!
//! * allowlist configured, but the request carries no determinable client
//!   address → DENY. A tenant that asked for origin restriction gets it,
//!   not a silent fall-through;
//! * the policy row cannot be loaded or decoded → DENY (503). An
//!   infrastructure hiccup must not disable a security control;
//! * no policy row or an empty allowlist → allow (the control is opt-in).
//!
//! # Client address source
//!
//! The socket peer and explicitly trusted proxies determine the client IP.
//! Forwarded headers from any other peer are ignored. All IP resolution is
//! owned by `trusted_proxy`; a missing peer cannot authenticate a header.

use std::net::IpAddr;

use axum::http::HeaderMap;
use sqlx::Row;

use bot_core::authorization::Decision;
use bot_core::tenant::OrganizationId;

use crate::api::ApiState;

pub use bot_core::config::parse_cidr;

use super::trusted_proxy::resolve_client_ip;

/// Is `ip` inside the network `network/prefix`? Address families must
/// match; a v4 client can never match a v6 rule or vice versa.
pub fn ip_matches_cidr(ip: IpAddr, network: IpAddr, prefix: u8) -> bool {
    match (ip, network) {
        (IpAddr::V4(ip), IpAddr::V4(network)) => {
            if prefix > 32 {
                return false;
            }
            let mask = if prefix == 0 {
                0u32
            } else {
                u32::MAX << (32 - prefix)
            };
            (u32::from(ip) & mask) == (u32::from(network) & mask)
        }
        (IpAddr::V6(ip), IpAddr::V6(network)) => {
            if prefix > 128 {
                return false;
            }
            let mask = if prefix == 0 {
                0u128
            } else {
                u128::MAX << (128 - prefix)
            };
            (u128::from(ip) & mask) == (u128::from(network) & mask)
        }
        // Family mismatch: never a match.
        _ => false,
    }
}

/// Enforce the organization's IP allowlist for one request.
///
/// Returns `Ok(())` when the request may proceed (no policy, empty
/// allowlist, or the client address is inside a configured CIDR) and a
/// denial [`Decision`] otherwise. Every refusal produced here is either
/// security-relevant (audited by the middleware's deny path) or a 503.
pub(crate) async fn enforce(
    state: &ApiState,
    organization_id: OrganizationId,
    headers: &HeaderMap,
    peer: Option<IpAddr>,
) -> Result<(), Decision> {
    let Some(db) = state.db.as_deref() else {
        // Without an attached database no allowlist can have been written
        // (the write path in `security.rs` requires it), so there is
        // nothing to enforce. The memory-only fixture has no policy rows.
        return Ok(());
    };

    let row =
        sqlx::query("SELECT ip_allowlist FROM tenant_security_policies WHERE organization_id = $1")
            .bind(organization_id.as_uuid())
            .fetch_optional(db.pool())
            .await
            .map_err(|error| {
                tracing::error!(
                    error = %error,
                    organization = %organization_id,
                    "IP allowlist policy could not be loaded; refusing the request (fail-closed)"
                );
                Decision::unavailable("the organization's IP allowlist could not be loaded")
            })?;

    let Some(row) = row else {
        // No policy row: the tenant never configured the control.
        return Ok(());
    };

    let allowlist: Vec<String> = row.try_get("ip_allowlist").map_err(|error| {
        tracing::error!(
            error = %error,
            organization = %organization_id,
            "IP allowlist policy could not be decoded; refusing the request (fail-closed)"
        );
        Decision::unavailable("the organization's IP allowlist could not be decoded")
    })?;

    if allowlist.is_empty() {
        return Ok(());
    }

    let Some(client_ip) = resolve_client_ip(peer, headers, &state.trusted_proxies) else {
        // Configured restriction + unverifiable origin = refuse. Silently
        // admitting the request would silently disable the control.
        tracing::warn!(
            organization = %organization_id,
            "request refused: IP allowlist configured but no client address is present"
        );
        return Err(Decision::permission(
            "the organization restricts access by IP allowlist and this request carries no verifiable client address",
        ));
    };

    let allowed = allowlist
        .iter()
        .filter_map(|entry| parse_cidr(entry))
        .any(|(network, prefix)| ip_matches_cidr(client_ip, network, prefix));

    if allowed {
        return Ok(());
    }

    tracing::warn!(
        organization = %organization_id,
        client_ip = %client_ip,
        "request refused: client address is outside the organization's IP allowlist"
    );
    Err(Decision::permission(
        "client address is not in the organization's IP allowlist",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn cidr_parsing_accepts_valid_entries_and_rejects_nonsense() {
        assert!(parse_cidr("10.0.0.0/8").is_some());
        assert!(parse_cidr("192.168.1.42/32").is_some());
        assert!(parse_cidr("2001:db8::/32").is_some());
        assert!(parse_cidr("0.0.0.0/0").is_some());
        assert!(parse_cidr("10.0.0.0/33").is_none(), "prefix too large");
        assert!(parse_cidr("2001:db8::/129").is_none());
        assert!(parse_cidr("10.0.0.0").is_none(), "missing prefix");
        assert!(parse_cidr("nope/8").is_none());
        assert!(parse_cidr("10.0.0.0/").is_none());
        assert!(parse_cidr("").is_none());
    }

    #[test]
    fn matching_respects_prefixes_and_families() {
        let net: IpAddr = "10.20.30.0".parse().unwrap();
        assert!(ip_matches_cidr("10.20.30.77".parse().unwrap(), net, 24));
        assert!(!ip_matches_cidr("10.20.31.1".parse().unwrap(), net, 24));
        assert!(ip_matches_cidr(
            "10.255.0.1".parse().unwrap(),
            "10.0.0.0".parse().unwrap(),
            8
        ));
        assert!(
            ip_matches_cidr("1.2.3.4".parse().unwrap(), "0.0.0.0".parse().unwrap(), 0),
            "a /0 matches everything of its family"
        );
        assert!(
            ip_matches_cidr(
                "10.20.30.42".parse().unwrap(),
                "10.20.30.42".parse().unwrap(),
                32
            ),
            "exact host match"
        );
        assert!(!ip_matches_cidr(
            "10.20.30.43".parse().unwrap(),
            "10.20.30.42".parse().unwrap(),
            32
        ));
        // Family mismatch never matches, even with permissive prefixes.
        assert!(!ip_matches_cidr(
            "10.0.0.1".parse().unwrap(),
            "2001:db8::".parse().unwrap(),
            0
        ));
        assert!(!ip_matches_cidr(
            "2001:db8::1".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
            0
        ));
        // IPv6 prefixes.
        assert!(ip_matches_cidr(
            "2001:db8:abcd::1".parse().unwrap(),
            "2001:db8::".parse().unwrap(),
            32
        ));
        assert!(!ip_matches_cidr(
            "2001:db9::1".parse().unwrap(),
            "2001:db8::".parse().unwrap(),
            32
        ));
    }

    #[test]
    fn client_ip_prefers_the_forwarded_origin_and_falls_back_to_real_ip() {
        let trusted =
            super::super::trusted_proxy::TrustedProxies::from_config(&["10.0.0.0/8".into()])
                .unwrap();
        let peer = Some("10.0.0.3".parse().unwrap());
        let h = headers(&[("x-forwarded-for", "192.0.2.7, 192.0.3.7, 10.0.0.2")]);
        assert_eq!(
            resolve_client_ip(peer, &h, &trusted),
            Some("192.0.3.7".parse().unwrap()),
            "rightmost untrusted entry is the originating client"
        );
        let h = headers(&[("x-real-ip", "192.0.2.9")]);
        assert_eq!(
            resolve_client_ip(peer, &h, &trusted),
            Some("192.0.2.9".parse().unwrap())
        );
        let h = headers(&[("x-forwarded-for", "garbage"), ("x-real-ip", "192.0.2.10")]);
        assert_eq!(
            resolve_client_ip(peer, &h, &trusted),
            Some("192.0.2.10".parse().unwrap()),
            "an unparsable forwarded chain falls back to x-real-ip only behind a trusted peer"
        );
        let h = headers(&[("x-forwarded-for", "  ")]);
        assert_eq!(resolve_client_ip(peer, &h, &trusted), None);
        assert_eq!(resolve_client_ip(peer, &HeaderMap::new(), &trusted), None);
    }

    #[test]
    fn spoofed_headers_never_choose_an_untrusted_peers_allowlist_identity() {
        let h = headers(&[("x-forwarded-for", "192.0.2.7"), ("x-real-ip", "192.0.2.7")]);
        let peer = Some("192.0.3.4".parse().unwrap());
        assert_eq!(resolve_client_ip(peer, &h, &Default::default()), peer);
        assert_eq!(resolve_client_ip(None, &h, &Default::default()), None);
    }
}
