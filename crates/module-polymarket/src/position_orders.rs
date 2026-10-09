//! Position-backed order construction (PROMPT 4/10 §D).
//!
//! Builds signed orders for PolyV2 markets — the ones that expose a
//! **position id** instead of a CTF token id — and the market-order
//! amount math for BOTH identifier kinds.
//!
//! What the official clients do that we mirror here (clob-client-v2
//! v1.2.0 is the spec's dependency reference):
//!
//! * the position id is carried through tick-size, order-book, fee and
//!   order-construction paths, and lands in the signed/wire `tokenId`
//!   field unchanged (the protocol kept the field name);
//! * a position order selects Exchange V3 signing automatically and
//!   disables neg-risk routing (see [`crate::exchange_v3`]);
//! * market buys are sized in **collateral** (pUSD): `amount` is what
//!   you spend; market sells are sized in **shares**: `amount` is what
//!   you sell. Limit orders are sized in shares at a price, exactly
//!   like the existing V2 path.
//!
//! All rounding goes through the SAME [`crate::orders::RoundConfig`]
//! machinery the token-backed path uses — a position market is still a
//! CLOB market with a tick grid.

use crate::error::{PolyError, PolyResult};
use crate::exchange_v3::{
    resolve_route, ExchangeEndpoint, ExchangeRoute, OrderAsset, V3_DOMAIN_VERSION,
    V3_EXCHANGE_ADDRESS,
};
use crate::orders::{
    limit_order_amounts, sign_order_bundle, OrderAmounts, OrderParams, RoundConfig,
};

/// The deployment's exchange contract addresses (from
/// `PolymarketConfig`).
#[derive(Debug, Clone)]
pub struct ExchangeAddresses {
    /// CLOB V2 standard exchange (`0x` address).
    pub v2_standard: String,
    /// CLOB V2 neg-risk exchange (`0x` address).
    pub v2_neg_risk: String,
}

impl ExchangeAddresses {
    /// Validate both V2 addresses are well-formed.
    pub fn validate(&self) -> PolyResult<()> {
        for (label, addr) in [
            ("v2_standard", &self.v2_standard),
            ("v2_neg_risk", &self.v2_neg_risk),
        ] {
            if addr.trim().len() != 42 || !addr.trim().starts_with("0x") {
                return Err(PolyError::invalid(format!(
                    "exchange address '{label}' is malformed: '{addr}'"
                )));
            }
        }
        Ok(())
    }
}

/// A position-backed LIMIT order request.
#[derive(Debug, Clone)]
pub struct PositionLimitOrder {
    /// The PolyV2 position id (decimal string).
    pub position_id: String,
    /// Buy (`true`) or sell.
    pub is_buy: bool,
    /// Size in shares (human units).
    pub size: f64,
    /// Limit price in (0, 1).
    pub price: f64,
    /// Tick size string, e.g. "0.01".
    pub tick_size: String,
    /// The market's `neg_risk` flag — IGNORED for position orders
    /// (kept in the request so callers do not need to pre-filter; the
    /// route resolution documents the override).
    pub market_neg_risk: bool,
    /// GTD expiry in unix seconds, or `None`/`0` for GTC.
    pub expiration: Option<u64>,
    /// Builder code (bytes32 hex) or `None`.
    pub builder_code: Option<String>,
}

/// A market order sized in the direction's native units (official
/// client semantics: buy = collateral to spend, sell = shares to sell).
#[derive(Debug, Clone)]
pub struct MarketOrderAmounts {
    /// The computed limit-order amounts (what actually gets signed).
    pub amounts: OrderAmounts,
    /// The execution type this size implies for a marketable order.
    pub order_type: &'static str,
}

/// Market-buy amount math: `amount` is collateral (pUSD) to spend and
/// `price` the execution-price hint. The maker gives the collateral
/// amount and takes `amount / price` shares (rounded to the tick's size
/// precision), mirroring the official client's fee-free sizing.
pub fn market_buy_amounts(
    amount_collateral: f64,
    price_hint: f64,
    tick_size: &str,
) -> PolyResult<MarketOrderAmounts> {
    if !amount_collateral.is_finite() || amount_collateral <= 0.0 {
        return Err(PolyError::invalid(format!(
            "market buy amount {amount_collateral} is not positive"
        )));
    }
    if !price_hint.is_finite() || price_hint <= 0.0 || price_hint >= 1.0 {
        return Err(PolyError::invalid(format!(
            "market buy price hint {price_hint} outside (0, 1)"
        )));
    }
    let cfg = RoundConfig::for_tick(tick_size)?;
    let shares = amount_collateral / price_hint;
    let amounts = limit_order_amounts(true, shares, price_hint, cfg);
    Ok(MarketOrderAmounts {
        amounts,
        order_type: "FAK",
    })
}

/// Market-sell amount math: `amount` is shares to sell at the
/// `price` hint — identical to a limit sell of that size.
pub fn market_sell_amounts(
    amount_shares: f64,
    price_hint: f64,
    tick_size: &str,
) -> PolyResult<MarketOrderAmounts> {
    if !amount_shares.is_finite() || amount_shares <= 0.0 {
        return Err(PolyError::invalid(format!(
            "market sell amount {amount_shares} is not positive"
        )));
    }
    if !price_hint.is_finite() || price_hint <= 0.0 || price_hint >= 1.0 {
        return Err(PolyError::invalid(format!(
            "market sell price hint {price_hint} outside (0, 1)"
        )));
    }
    let cfg = RoundConfig::for_tick(tick_size)?;
    let amounts = limit_order_amounts(false, amount_shares, price_hint, cfg);
    Ok(MarketOrderAmounts {
        amounts,
        order_type: "FAK",
    })
}

/// The resolved signing plan for one order: which route, which
/// endpoint, and the `OrderParams` ready for
/// [`sign_order_bundle`].
pub struct SigningPlan<'a> {
    /// The asset being traded (token or position).
    pub asset: &'a OrderAsset,
    /// The resolved exchange route.
    pub route: ExchangeRoute,
    /// The EIP-712 endpoint (domain version + verifying contract).
    pub endpoint: ExchangeEndpoint,
    /// `true` when the order must be posted with neg-risk = false
    /// (position orders and non-neg-risk token orders alike — the
    /// bundle carries this for the wire).
    pub neg_risk: bool,
}

/// Resolve the signing plan for an asset against the deployment's
/// exchange addresses: position orders are pinned to Exchange V3 (with
/// neg-risk OFF), token orders keep the V2 standard/neg-risk selection.
pub fn signing_plan<'a>(
    asset: &'a OrderAsset,
    market_neg_risk: bool,
    addresses: &ExchangeAddresses,
) -> PolyResult<SigningPlan<'a>> {
    addresses.validate()?;
    asset.validate()?;
    let route = resolve_route(asset, market_neg_risk);
    let endpoint =
        ExchangeEndpoint::for_route(route, &addresses.v2_standard, &addresses.v2_neg_risk)?;
    endpoint.validate()?;
    // Neg-risk only ever applies to V2 token orders; the V3 route has
    // no neg-risk variant, so the wire flag is forced off.
    let neg_risk = route == ExchangeRoute::V2NegRisk;
    Ok(SigningPlan {
        asset,
        route,
        endpoint,
        neg_risk,
    })
}

/// Build and sign a position-backed limit order.
///
/// The position id is carried in the signed `tokenId` field (protocol
/// field name preserved), the domain is Exchange V3
/// (`version "3"`, canonical verifying contract), and the bundle's
/// `neg_risk` is `false` regardless of the market flag.
pub fn sign_position_limit_order(
    key: &k256::ecdsa::SigningKey,
    req: &PositionLimitOrder,
    chain_id: u64,
    signature_type: u8,
    funder: Option<&str>,
    addresses: &ExchangeAddresses,
    created_at_ms: u64,
) -> PolyResult<crate::orders::SignedOrderBundle> {
    let asset = OrderAsset::exactly_one(None, Some(&req.position_id))?;
    let plan = signing_plan(&asset, req.market_neg_risk, addresses)?;
    // A position order that did NOT resolve to V3 is a programming
    // error in the routing table — refuse rather than sign against the
    // wrong verifier.
    if plan.route != ExchangeRoute::V3 {
        return Err(PolyError::invalid(format!(
            "position order resolved to {} — expected v3",
            plan.route.as_str()
        )));
    }
    let params = OrderParams {
        token_id: asset.id(),
        is_buy: req.is_buy,
        size: req.size,
        price: req.price,
        tick_size: &req.tick_size,
        neg_risk: false,
        chain_id,
        domain_version: V3_DOMAIN_VERSION,
        exchange_address: &plan.endpoint.verifying_contract,
        // V3 has no neg-risk variant; the standard slot carries the V3
        // contract and the neg-risk flag is off, so this arm is never
        // selected.
        neg_risk_exchange_address: &plan.endpoint.verifying_contract,
        signature_type,
        funder,
        expiration_timestamp: req.expiration.unwrap_or(0),
        created_at_ms,
        builder_code: req.builder_code.as_deref(),
    };
    let bundle = sign_order_bundle(key, &params)?;
    // Post-sign invariants: the bundle must be signed against the V3
    // contract and flagged non-neg-risk.
    if bundle.verifying_contract != V3_EXCHANGE_ADDRESS {
        return Err(PolyError::invalid(format!(
            "position order signed against {} — expected the Exchange V3 contract {V3_EXCHANGE_ADDRESS}",
            bundle.verifying_contract
        )));
    }
    if bundle.neg_risk {
        return Err(PolyError::invalid(
            "position order must never be flagged neg-risk (Exchange V3 has no neg-risk variant)",
        ));
    }
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eip712::{self, SIDE_BUY, SIDE_SELL};

    const V2_STD: &str = "0xE111180000d2663C0091e4f400237545B87B996B";
    const V2_NR: &str = "0xe2222d279d744050d28e00520010520000310F59";

    fn addresses() -> ExchangeAddresses {
        ExchangeAddresses {
            v2_standard: V2_STD.into(),
            v2_neg_risk: V2_NR.into(),
        }
    }

    fn key() -> k256::ecdsa::SigningKey {
        // Deterministic test key (test-only; never a funded key).
        k256::ecdsa::SigningKey::from_bytes(&[7u8; 32].into()).unwrap()
    }

    #[test]
    fn a_position_order_signs_against_exchange_v3() {
        let req = PositionLimitOrder {
            position_id: "456".into(),
            is_buy: true,
            size: 100.0,
            price: 0.4,
            tick_size: "0.01".into(),
            market_neg_risk: true, // must be ignored
            expiration: None,
            builder_code: None,
        };
        let bundle =
            sign_position_limit_order(&key(), &req, 137, 0, None, &addresses(), 1_713_398_400_000)
                .unwrap();
        assert_eq!(bundle.verifying_contract, V3_EXCHANGE_ADDRESS);
        assert!(!bundle.neg_risk);
        // the position id rides in the tokenId field unchanged.
        assert_eq!(bundle.order.token_id, "456");
        // timestamp is the creation time in MILLISECONDS (V2/V3
        // semantics: uniqueness field, replaces the removed nonce).
        assert_eq!(bundle.order.timestamp, "1713398400000");
        // GTC: expiration not folded into the signed struct.
        assert_eq!(bundle.order.metadata, "");
    }

    #[test]
    fn derived_order_id_differs_between_signings_at_different_times() {
        let req = PositionLimitOrder {
            position_id: "456".into(),
            is_buy: true,
            size: 100.0,
            price: 0.4,
            tick_size: "0.01".into(),
            market_neg_risk: false,
            expiration: None,
            builder_code: None,
        };
        let a =
            sign_position_limit_order(&key(), &req, 137, 0, None, &addresses(), 1_713_398_400_000)
                .unwrap();
        let b =
            sign_position_limit_order(&key(), &req, 137, 0, None, &addresses(), 1_713_398_400_001)
                .unwrap();
        // timestamp is the per-address uniqueness field: two creations
        // are two orders, exactly like the nonce it replaced.
        assert_ne!(a.order.timestamp, b.order.timestamp);
        assert_ne!(a.derived_order_id().unwrap(), b.derived_order_id().unwrap());
    }

    #[test]
    fn token_orders_keep_the_v2_route_and_addresses() {
        let asset = OrderAsset::exactly_one(Some("102936"), None).unwrap();
        let plan = signing_plan(&asset, false, &addresses()).unwrap();
        assert_eq!(plan.route, ExchangeRoute::V2Standard);
        assert_eq!(plan.endpoint.domain_version, eip712::V2_DOMAIN_VERSION);
        assert_eq!(plan.endpoint.verifying_contract, V2_STD);
        assert!(!plan.neg_risk);

        let neg = signing_plan(&asset, true, &addresses()).unwrap();
        assert_eq!(neg.route, ExchangeRoute::V2NegRisk);
        assert_eq!(neg.endpoint.verifying_contract, V2_NR);
        assert!(neg.neg_risk);
    }

    #[test]
    fn malformed_deployment_addresses_are_refused() {
        let bad = ExchangeAddresses {
            v2_standard: "0x1234".into(),
            v2_neg_risk: V2_NR.into(),
        };
        let asset = OrderAsset::exactly_one(Some("1"), None).unwrap();
        assert!(signing_plan(&asset, false, &bad).is_err());
    }

    #[test]
    fn market_amounts_follow_the_official_direction_semantics() {
        // buy: amount is collateral to spend.
        let buy = market_buy_amounts(100.0, 0.5, "0.01").unwrap();
        assert_eq!(buy.order_type, "FAK");
        // 100 collateral at 0.5 → 200 shares: maker gives 100e6, takes
        // 200e6.
        assert_eq!(buy.amounts.maker_amount, 100_000_000);
        assert_eq!(buy.amounts.taker_amount, 200_000_000);
        assert_eq!(buy.amounts.side, SIDE_BUY);

        // sell: amount is shares.
        let sell = market_sell_amounts(150.0, 0.25, "0.01").unwrap();
        assert_eq!(sell.amounts.maker_amount, 150_000_000);
        assert_eq!(sell.amounts.taker_amount, 37_500_000);
        assert_eq!(sell.amounts.side, SIDE_SELL);
    }

    #[test]
    fn market_amounts_reject_non_positive_inputs() {
        assert!(market_buy_amounts(0.0, 0.5, "0.01").is_err());
        assert!(market_buy_amounts(10.0, 0.0, "0.01").is_err());
        assert!(market_buy_amounts(10.0, 1.0, "0.01").is_err());
        assert!(market_sell_amounts(0.0, 0.5, "0.01").is_err());
        assert!(market_sell_amounts(10.0, 1.5, "0.01").is_err());
    }

    #[test]
    fn position_requests_with_empty_ids_are_refused() {
        let req = PositionLimitOrder {
            position_id: "  ".into(),
            is_buy: true,
            size: 1.0,
            price: 0.5,
            tick_size: "0.01".into(),
            market_neg_risk: false,
            expiration: None,
            builder_code: None,
        };
        assert!(sign_position_limit_order(&key(), &req, 137, 0, None, &addresses(), 1).is_err());
    }
}
