//! The sole client-IP resolver for tenant allowlists and per-IP rate limits.
//! Header values are meaningful only behind an explicitly trusted socket peer.

use std::net::IpAddr;

use axum::http::HeaderMap;

use super::ip_allowlist::{ip_matches_cidr, parse_cidr};

#[derive(Debug, Clone, Default)]
pub struct TrustedProxies {
    networks: Vec<(IpAddr, u8)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedProxyConfigError(String);

impl std::fmt::Display for TrustedProxyConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid api.trusted_proxies CIDR: {:?}", self.0)
    }
}

impl std::error::Error for TrustedProxyConfigError {}

impl TrustedProxies {
    /// Invalid entries are startup errors, never silently dropped rules.
    pub fn from_config(cidrs: &[String]) -> Result<Self, TrustedProxyConfigError> {
        let networks = cidrs
            .iter()
            .map(|value| parse_cidr(value).ok_or_else(|| TrustedProxyConfigError(value.clone())))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { networks })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        self.networks
            .iter()
            .any(|&(network, prefix)| ip_matches_cidr(ip, network, prefix))
    }

    pub fn is_empty(&self) -> bool {
        self.networks.is_empty()
    }
}

/// Walk the complete forwarded chain from right to left, discarding only
/// trusted hops. No peer means no header trust. A malformed chain may use
/// X-Real-IP only when the immediate peer is trusted; absent verifiable
/// origin behind that peer returns None (allowlist callers fail closed).
pub fn resolve_client_ip(
    peer: Option<IpAddr>,
    headers: &HeaderMap,
    trusted: &TrustedProxies,
) -> Option<IpAddr> {
    let peer = peer?;
    if !trusted.contains(peer) {
        return Some(peer);
    }

    // Multiple header fields form one ordered chain, not independent chains
    // from which a caller could select a convenient client address.
    let values = headers.get_all("x-forwarded-for");
    let chain = values
        .iter()
        .map(|value| value.to_str().ok())
        .collect::<Option<Vec<_>>>()
        .and_then(|values| {
            values
                .iter()
                .flat_map(|value| value.split(','))
                .map(|value| value.trim().parse::<IpAddr>().ok())
                .collect::<Option<Vec<_>>>()
        });
    if let Some(chain) = chain.filter(|chain| !chain.is_empty()) {
        return chain.into_iter().rev().find(|ip| !trusted.contains(*ip));
    }

    let real_ips = headers.get_all("x-real-ip");
    let mut values = real_ips.iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    value.to_str().ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trust() -> TrustedProxies {
        TrustedProxies::from_config(&["10.0.0.0/8".into(), "2001:db8:ffff::/48".into()]).unwrap()
    }

    fn forwarded(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", value.parse().unwrap());
        headers
    }

    #[test]
    fn untrusted_peer_cannot_spoof_an_allowlisted_ip() {
        let peer = Some("192.0.3.4".parse().unwrap());
        let mut headers = forwarded("192.0.2.1");
        headers.insert("x-real-ip", "192.0.2.1".parse().unwrap());
        assert_eq!(resolve_client_ip(peer, &headers, &trust()), peer);
    }

    #[test]
    fn chooses_rightmost_untrusted_hop_not_attacker_leftmost() {
        let headers = forwarded("192.0.2.1, 192.0.3.4, 10.1.1.1");
        assert_eq!(
            resolve_client_ip(Some("10.2.2.2".parse().unwrap()), &headers, &trust()),
            Some("192.0.3.4".parse().unwrap())
        );
    }

    #[test]
    fn empty_trust_ignores_both_forwarded_headers() {
        let peer = Some("10.2.2.2".parse().unwrap());
        let mut headers = forwarded("192.0.2.1");
        headers.insert("x-real-ip", "192.0.2.9".parse().unwrap());
        assert_eq!(
            resolve_client_ip(peer, &headers, &TrustedProxies::default()),
            peer
        );
        assert_eq!(resolve_client_ip(None, &headers, &trust()), None);
    }

    #[test]
    fn supports_ipv6_and_mixed_families_without_cross_family_trust() {
        assert_eq!(
            resolve_client_ip(
                Some("2001:db8:ffff::1".parse().unwrap()),
                &forwarded("2001:db8::1, 2001:db8:ffff::2"),
                &trust(),
            ),
            Some("2001:db8::1".parse().unwrap())
        );
        assert!(!trust().contains("::ffff:10.0.0.1".parse().unwrap()));
        assert_eq!(
            resolve_client_ip(
                Some("10.0.0.1".parse().unwrap()),
                &forwarded("2001:db8::5"),
                &trust()
            ),
            Some("2001:db8::5".parse().unwrap())
        );
    }

    #[test]
    fn missing_malformed_and_all_trusted_chains_fail_closed() {
        let peer = Some("10.0.0.1".parse().unwrap());
        for headers in [
            HeaderMap::new(),
            forwarded("garbage"),
            forwarded("10.0.0.2, 10.0.0.3"),
        ] {
            assert_eq!(resolve_client_ip(peer, &headers, &trust()), None);
        }
        let mut headers = forwarded("invalid, 192.0.2.5");
        headers.insert("x-real-ip", "192.0.3.5".parse().unwrap());
        assert_eq!(
            resolve_client_ip(peer, &headers, &trust()),
            Some("192.0.3.5".parse().unwrap())
        );
        headers.append("x-real-ip", "192.0.2.6".parse().unwrap());
        assert_eq!(resolve_client_ip(peer, &headers, &trust()), None);
    }

    #[test]
    fn multiple_xff_fields_are_one_chain() {
        let mut headers = forwarded("192.0.2.1");
        headers.append("x-forwarded-for", "192.0.3.4, 10.0.0.2".parse().unwrap());
        assert_eq!(
            resolve_client_ip(Some("10.0.0.1".parse().unwrap()), &headers, &trust()),
            Some("192.0.3.4".parse().unwrap())
        );
    }

    #[test]
    fn invalid_configuration_is_not_silently_accepted() {
        for entry in ["", "10.0.0.1", "10.0.0.0/33", "::/129"] {
            assert!(TrustedProxies::from_config(&[entry.into()]).is_err());
        }
    }
}
