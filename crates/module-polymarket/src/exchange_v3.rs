//! Exchange V3 routing and EIP-712 domain (PROMPT 4/10 §D).
//!
//! Polymarket's PolyV2 markets expose **position IDs** instead of CTF
//! token IDs. An order that identifies a position is signed against
//! **Exchange V3** — same 11-field `Order` struct as CLOB V2, but a
//! different EIP-712 domain (version `"3"` and a dedicated verifying
//! contract), and **neg-risk routing does not apply** (V3 has one
//! exchange contract; there is no neg-risk variant to select).
//!
//! Official semantics this module implements (see
//! `PROMPT-4-POLYMARKET-RESEARCH.md` for the source table):
//!
//! * an order carries **exactly one** of `tokenID` or `positionID`.
//!   The official TypeScript client (clob-client-v2 v1.2.0, PR #104 +
//!   #110) added exactly-one validation after a caller could pass both
//!   and silently get V3; we validate at RUNTIME, not just at the type
//!   level;
//! * `positionID` ⇒ Exchange V3 signing, automatically, and any
//!   requested neg-risk routing is IGNORED for that order;
//! * token-ID orders keep the existing CLOB V2 selection (standard vs
//!   neg-risk exchange by the market's `neg_risk` flag);
//! * the signed and wire field name stays `tokenId` for BOTH kinds of
//!   identifier — the position id is simply carried in it.
//!
//! Docs discrepancy, recorded honestly: the Combos/RFQ page labels the
//! V3 `timestamp` field `<unix_seconds>` while the CLOB V2 migration
//! guide and every `POST /order` example use **milliseconds**. The
//! dependency reference named by the spec (clob-client-v2 v1.2.0) uses
//! milliseconds for CLOB orders, so the V3 signing path here reuses the
//! exact same millisecond `timestamp` the V2 path already produces —
//! one code path, one unit, no silent divergence.

use crate::error::{PolyError, PolyResult};

/// EIP-712 Exchange domain version for V3 position-backed orders.
pub const V3_DOMAIN_VERSION: &str = "3";

/// The Exchange V3 verifying contract on Polygon (chain id 137).
pub const V3_EXCHANGE_ADDRESS: &str = "0xe3333700cA9d93003F00f0F71f8515005F6c00Aa";

/// The chain id Polymarket's exchanges deploy on (Polygon mainnet).
pub const POLYGON_CHAIN_ID: u64 = 137;

/// The asset an order trades. Exactly one identifier is present —
/// the type system cannot express "both" and the runtime constructor
/// enforces it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OrderAsset {
    /// A CTF ERC-1155 outcome token id (decimal string) — the classic
    /// CLOB V2 identifier.
    Token {
        /// The CTF token id.
        token_id: String,
    },
    /// A PolyV2 position id (decimal string) — selects Exchange V3.
    Position {
        /// The position id.
        position_id: String,
    },
}

impl OrderAsset {
    /// Construct from optional token/position identifiers, enforcing
    /// **exactly one** non-empty value at runtime. `None`, `Some("")`
    /// and "both set" are all refused with explicit errors — the
    /// official client's own validation history (TS types alone let
    /// `{tokenID, positionID: null}` silently sign against V3) is why
    /// this exists here too.
    pub fn exactly_one(
        token_id: Option<&str>,
        position_id: Option<&str>,
    ) -> PolyResult<OrderAsset> {
        let token = token_id.map(str::trim).filter(|s| !s.is_empty());
        let position = position_id.map(str::trim).filter(|s| !s.is_empty());
        match (token, position) {
            (Some(t), None) => Ok(OrderAsset::Token {
                token_id: t.to_string(),
            }),
            (None, Some(p)) => Ok(OrderAsset::Position {
                position_id: p.to_string(),
            }),
            (Some(_), Some(_)) => Err(PolyError::invalid(
                "order asset is ambiguous: pass exactly one of token_id or position_id (both set)",
            )),
            (None, None) => Err(PolyError::invalid(
                "order asset is missing: pass exactly one of token_id or position_id (neither set)",
            )),
        }
    }

    /// The identifier this asset carries — this is the value placed in
    /// the signed/wire `tokenId` field for BOTH variants (the protocol
    /// kept the field name; position ids ride in it).
    pub fn id(&self) -> &str {
        match self {
            OrderAsset::Token { token_id } => token_id,
            OrderAsset::Position { position_id } => position_id,
        }
    }

    /// Whether this is a PolyV2 position-backed order.
    pub fn is_position_backed(&self) -> bool {
        matches!(self, OrderAsset::Position { .. })
    }

    /// Whether this asset must be signed against Exchange V3.
    pub fn requires_exchange_v3(&self) -> bool {
        self.is_position_backed()
    }

    /// Validate the identifier is a decimal uint256 (position ids are
    /// large decimal numbers exactly like CTF token ids).
    pub fn validate(&self) -> PolyResult<()> {
        let id = self.id();
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
            return Err(PolyError::invalid(format!(
                "order asset id '{id}' is not a decimal uint256"
            )));
        }
        Ok(())
    }
}

/// Which exchange contract an order is signed against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExchangeRoute {
    /// CLOB V2 standard exchange (token-backed, neg-risk false).
    V2Standard,
    /// CLOB V2 neg-risk exchange (token-backed, neg-risk true).
    V2NegRisk,
    /// Exchange V3 (position-backed; one contract, no neg-risk
    /// variant).
    V3,
}

impl ExchangeRoute {
    /// Stable machine-readable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            ExchangeRoute::V2Standard => "v2_standard",
            ExchangeRoute::V2NegRisk => "v2_neg_risk",
            ExchangeRoute::V3 => "v3",
        }
    }

    /// The EIP-712 Exchange domain version for this route.
    pub fn domain_version(&self) -> &'static str {
        match self {
            ExchangeRoute::V2Standard | ExchangeRoute::V2NegRisk => {
                crate::eip712::V2_DOMAIN_VERSION
            }
            ExchangeRoute::V3 => V3_DOMAIN_VERSION,
        }
    }
}

/// Resolve the signing route for one order.
///
/// * position-backed ⇒ **always V3** — a `neg_risk` flag carried from
///   market metadata is IGNORED for position orders (the official
///   Python client documents the same: "position_id … ignores neg_risk
///   and any requested exchange version");
/// * token-backed ⇒ the existing V2 behaviour: neg-risk exchange when
///   the market is neg-risk, standard exchange otherwise.
pub fn resolve_route(asset: &OrderAsset, market_neg_risk: bool) -> ExchangeRoute {
    match asset {
        OrderAsset::Position { .. } => ExchangeRoute::V3,
        OrderAsset::Token { .. } => {
            if market_neg_risk {
                ExchangeRoute::V2NegRisk
            } else {
                ExchangeRoute::V2Standard
            }
        }
    }
}

/// The EIP-712 signing endpoint for one route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExchangeEndpoint {
    /// Domain version ("2" for V2 routes, "3" for V3).
    pub domain_version: &'static str,
    /// The verifying contract (`0x` address).
    pub verifying_contract: String,
    /// The route this endpoint serves.
    pub route: ExchangeRoute,
}

impl ExchangeEndpoint {
    /// The endpoint for a resolved route, given the deployment's V2
    /// contract addresses (V3 has a fixed canonical contract).
    pub fn for_route(
        route: ExchangeRoute,
        v2_standard_address: &str,
        v2_neg_risk_address: &str,
    ) -> PolyResult<ExchangeEndpoint> {
        match route {
            ExchangeRoute::V2Standard => Ok(ExchangeEndpoint {
                domain_version: route.domain_version(),
                verifying_contract: v2_standard_address.trim().to_string(),
                route,
            }),
            ExchangeRoute::V2NegRisk => Ok(ExchangeEndpoint {
                domain_version: route.domain_version(),
                verifying_contract: v2_neg_risk_address.trim().to_string(),
                route,
            }),
            ExchangeRoute::V3 => Ok(ExchangeEndpoint {
                domain_version: V3_DOMAIN_VERSION,
                verifying_contract: V3_EXCHANGE_ADDRESS.to_string(),
                route,
            }),
        }
    }

    /// Validate the endpoint is usable for signing (non-empty,
    /// `0x`-prefixed address).
    pub fn validate(&self) -> PolyResult<()> {
        if self.verifying_contract.len() != 42 || !self.verifying_contract.starts_with("0x") {
            return Err(PolyError::invalid(format!(
                "exchange endpoint for {} has a malformed verifying contract '{}'",
                self.route.as_str(),
                self.verifying_contract
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V2_STD: &str = "0xE111180000d2663C0091e4f400237545B87B996B";
    const V2_NR: &str = "0xe2222d279d744050d28e00520010520000310F59";

    #[test]
    fn exactly_one_rejects_neither_and_both() {
        assert!(OrderAsset::exactly_one(None, None).is_err());
        assert!(OrderAsset::exactly_one(Some(""), Some("")).is_err());
        // both set — the exact ambiguity the official client had to
        // patch (#110); must be refused, never silently resolved.
        let err = OrderAsset::exactly_one(Some("123"), Some("456")).unwrap_err();
        assert!(err.to_string().contains("ambiguous"));
        // whitespace-only is treated as unset.
        assert!(OrderAsset::exactly_one(Some("   "), None).is_err());
    }

    #[test]
    fn exactly_one_accepts_each_single_form() {
        let token = OrderAsset::exactly_one(Some(" 102936 "), None).unwrap();
        assert_eq!(
            token,
            OrderAsset::Token {
                token_id: "102936".into()
            }
        );
        assert!(!token.is_position_backed());
        assert_eq!(token.id(), "102936");

        let position = OrderAsset::exactly_one(None, Some("456")).unwrap();
        assert_eq!(
            position,
            OrderAsset::Position {
                position_id: "456".into()
            }
        );
        assert!(position.is_position_backed());
        assert!(position.requires_exchange_v3());
        // the position id rides in the tokenId field unchanged.
        assert_eq!(position.id(), "456");
    }

    #[test]
    fn token_orders_keep_v2_routing() {
        let token = OrderAsset::exactly_one(Some("123"), None).unwrap();
        assert_eq!(resolve_route(&token, false), ExchangeRoute::V2Standard);
        assert_eq!(resolve_route(&token, true), ExchangeRoute::V2NegRisk);
    }

    #[test]
    fn position_orders_force_v3_and_ignore_neg_risk() {
        let position = OrderAsset::exactly_one(None, Some("456")).unwrap();
        assert_eq!(resolve_route(&position, false), ExchangeRoute::V3);
        // neg-risk must be IGNORED for position orders, not honored.
        assert_eq!(resolve_route(&position, true), ExchangeRoute::V3);
    }

    #[test]
    fn v3_endpoint_uses_the_canonical_domain() {
        let endpoint = ExchangeEndpoint::for_route(ExchangeRoute::V3, V2_STD, V2_NR).unwrap();
        assert_eq!(endpoint.domain_version, "3");
        assert_eq!(
            endpoint.verifying_contract,
            "0xe3333700cA9d93003F00f0F71f8515005F6c00Aa"
        );
        endpoint.validate().unwrap();
    }

    #[test]
    fn v2_endpoints_use_the_configured_addresses() {
        let std = ExchangeEndpoint::for_route(ExchangeRoute::V2Standard, V2_STD, V2_NR).unwrap();
        assert_eq!(std.domain_version, crate::eip712::V2_DOMAIN_VERSION);
        assert_eq!(std.verifying_contract, V2_STD);
        let nr = ExchangeEndpoint::for_route(ExchangeRoute::V2NegRisk, V2_STD, V2_NR).unwrap();
        assert_eq!(nr.verifying_contract, V2_NR);
        assert_eq!(nr.domain_version, "2");
    }

    #[test]
    fn malformed_endpoint_addresses_are_refused() {
        let bad = ExchangeEndpoint {
            domain_version: "3",
            verifying_contract: "0x1234".into(),
            route: ExchangeRoute::V3,
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn asset_ids_must_be_decimal() {
        let bad = OrderAsset::Position {
            position_id: "0xabc".into(),
        };
        assert!(bad.validate().is_err());
        let good = OrderAsset::Position {
            position_id: "456".into(),
        };
        good.validate().unwrap();
    }
}
