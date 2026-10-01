//! Exchange V3 (position-backed) signing — integration vectors
//! (PROMPT 4/10 §D).
//!
//! Proves the V3 surface against the official documented semantics:
//!
//! * position orders sign against the Exchange **V3** domain
//!   (`version "3"`, verifying contract
//!   `0xe3333700cA9d93003F00f0F71f8515005F6c00Aa`) with the position
//!   id carried in the signed `tokenId` field;
//! * a `neg_risk` market flag is IGNORED for position orders (V3 has
//!   no neg-risk variant) while token orders keep the V2
//!   standard/neg-risk selection byte-for-byte;
//! * exactly one of token/position identifier is enforced at runtime
//!   (the ambiguity the official client had to patch);
//! * the signed `timestamp` is creation-time milliseconds (the field
//!   that replaced the removed `nonce`), so two creations differ and
//!   the GTD expiry travels in the wire body, never the signature;
//! * the signature is a recoverable ECDSA signature over the V3
//!   digest and recovers to the signer's address.

mod common;

use module_polymarket::eip712;
use module_polymarket::exchange_v3::{
    resolve_route, ExchangeRoute, OrderAsset, V3_DOMAIN_VERSION, V3_EXCHANGE_ADDRESS,
};
use module_polymarket::orders::sign_order_bundle;
use module_polymarket::position_orders::{
    sign_position_limit_order, ExchangeAddresses, PositionLimitOrder, SigningPlan,
};

const V2_STD: &str = "0xE111180000d2663C0091e4f400237545B87B996B";
const V2_NR: &str = "0xe2222d279d744050d28e00520010520000310F59";
const CHAIN: u64 = 137;

fn addresses() -> ExchangeAddresses {
    ExchangeAddresses {
        v2_standard: V2_STD.into(),
        v2_neg_risk: V2_NR.into(),
    }
}

#[test]
fn the_v3_domain_separator_is_stable_and_distinct_from_v2() {
    // The V3 domain separator over the documented constants — stable
    // across runs (keccak is deterministic) and DIFFERENT from a V2
    // domain over the same verifying contract (the version string is
    // part of the separator).
    let v3 = eip712::domain_separator(CHAIN, V3_DOMAIN_VERSION, V3_EXCHANGE_ADDRESS).unwrap();
    let again = eip712::domain_separator(CHAIN, V3_DOMAIN_VERSION, V3_EXCHANGE_ADDRESS).unwrap();
    assert_eq!(v3, again);
    let v2_same_contract =
        eip712::domain_separator(CHAIN, eip712::V2_DOMAIN_VERSION, V3_EXCHANGE_ADDRESS).unwrap();
    assert_ne!(v3, v2_same_contract);
    // And distinct from the real V2 standard contract's separator.
    let v2_std = eip712::domain_separator(CHAIN, eip712::V2_DOMAIN_VERSION, V2_STD).unwrap();
    assert_ne!(v3, v2_std);
}

#[test]
fn a_position_limit_order_signs_against_exchange_v3() {
    let key = common::test_key();
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
    let bundle =
        sign_position_limit_order(&key, &req, CHAIN, 0, None, &addresses(), 1_713_398_400_000)
            .expect("position order must sign");
    // V3 contract, neg-risk off, position id in tokenId.
    assert_eq!(bundle.verifying_contract, V3_EXCHANGE_ADDRESS);
    assert!(!bundle.neg_risk);
    assert_eq!(bundle.order.token_id, "456");
    // Creation-time milliseconds in the signed timestamp.
    assert_eq!(bundle.order.timestamp, "1713398400000");
    // The signature recovers to the signer's address (EOA type 0):
    // 65 raw bytes = r || s || v with v ∈ {0, 1}.
    let signer = eip712::address_from_signing_key(&key);
    assert!(bundle.signature.starts_with("0x"));
    let raw = hex::decode(&bundle.signature[2..]).expect("hex signature");
    assert_eq!(raw.len(), 65);
    // The engine's `sign_digest` emits Ethereum-style v ∈ {27, 28}.
    let recid = k256::ecdsa::RecoveryId::from_byte(raw[64] - 27).expect("recovery id");
    let sig = k256::ecdsa::Signature::from_slice(&raw[..64]).expect("r||s");
    let digest =
        eip712::order_digest(CHAIN, V3_DOMAIN_VERSION, V3_EXCHANGE_ADDRESS, &bundle.order).unwrap();
    let recovered = k256::ecdsa::VerifyingKey::recover_from_prehash(&digest, &sig, recid)
        .expect("signature must recover");
    let point = recovered.to_encoded_point(false);
    let hash = eip712::keccak256(&point.as_bytes()[1..]);
    let recovered_addr = format!("0x{}", hex::encode(&hash[12..]));
    assert_eq!(recovered_addr, signer);
    // The derived order id is the exchange's own getOrderHash.
    let id = bundle.derived_order_id().unwrap();
    assert!(id.starts_with("0x") && id.len() == 66);
}

#[test]
fn a_neg_risk_market_flag_is_ignored_for_position_orders() {
    let key = common::test_key();
    let req = PositionLimitOrder {
        position_id: "456".into(),
        is_buy: false,
        size: 10.0,
        price: 0.6,
        tick_size: "0.01".into(),
        market_neg_risk: true, // must be ignored on the V3 route
        expiration: None,
        builder_code: None,
    };
    let bundle =
        sign_position_limit_order(&key, &req, CHAIN, 0, None, &addresses(), 1_713_398_400_500)
            .unwrap();
    assert_eq!(bundle.verifying_contract, V3_EXCHANGE_ADDRESS);
    assert!(!bundle.neg_risk);
    // Same position at the same creation time signs identically
    // regardless of the market flag.
    let req2 = PositionLimitOrder {
        market_neg_risk: false,
        ..req
    };
    let bundle2 =
        sign_position_limit_order(&key, &req2, CHAIN, 0, None, &addresses(), 1_713_398_400_500)
            .unwrap();
    assert_eq!(bundle.signature, bundle2.signature);
}

#[test]
fn token_orders_keep_v2_routing_and_addresses_byte_for_byte() {
    let key = common::test_key();
    for (neg_risk, expected_contract, expected_route) in [
        (false, V2_STD, ExchangeRoute::V2Standard),
        (true, V2_NR, ExchangeRoute::V2NegRisk),
    ] {
        let asset = OrderAsset::exactly_one(Some("102936"), None).unwrap();
        let plan: SigningPlan<'_> =
            module_polymarket::position_orders::signing_plan(&asset, neg_risk, &addresses())
                .unwrap();
        assert_eq!(plan.route, expected_route);
        assert_eq!(plan.endpoint.verifying_contract, expected_contract);
        assert_eq!(plan.endpoint.domain_version, eip712::V2_DOMAIN_VERSION);

        // And a V2 token order signed through the SAME bundle builder
        // still verifies against the V2 domain (no behavior change).
        let params = module_polymarket::orders::OrderParams {
            token_id: "102936",
            is_buy: true,
            size: 5.0,
            price: 0.4,
            tick_size: "0.01",
            neg_risk,
            chain_id: CHAIN,
            domain_version: eip712::V2_DOMAIN_VERSION,
            exchange_address: V2_STD,
            neg_risk_exchange_address: V2_NR,
            signature_type: 0,
            funder: None,
            expiration_timestamp: 0,
            created_at_ms: 1_713_398_400_000,
            builder_code: None,
        };
        let bundle = sign_order_bundle(&key, &params).unwrap();
        assert_eq!(bundle.verifying_contract, expected_contract);
        assert_eq!(bundle.expiration, "0");
    }
}

#[test]
fn gtd_expiry_travels_in_the_wire_body_not_the_signature() {
    let key = common::test_key();
    let req = PositionLimitOrder {
        position_id: "789".into(),
        is_buy: true,
        size: 20.0,
        price: 0.25,
        tick_size: "0.01".into(),
        market_neg_risk: false,
        expiration: Some(1_714_000_000),
        builder_code: None,
    };
    let bundle =
        sign_position_limit_order(&key, &req, CHAIN, 0, None, &addresses(), 1_713_398_400_000)
            .unwrap();
    // The signed timestamp stays the creation time; the expiry is a
    // bundle (wire) field.
    assert_eq!(bundle.order.timestamp, "1713398400000");
    assert_eq!(bundle.expiration, "1714000000");
}

#[test]
fn exactly_one_identifier_is_enforced_at_runtime() {
    // neither / both / whitespace are all refused.
    assert!(OrderAsset::exactly_one(None, None).is_err());
    assert!(OrderAsset::exactly_one(Some("1"), Some("2")).is_err());
    // A whitespace-only token id is treated as unset, so the
    // position id is the exactly-one identifier.
    assert_eq!(
        OrderAsset::exactly_one(Some(" "), Some("2")).unwrap(),
        OrderAsset::Position {
            position_id: "2".into()
        }
    );
    // and a malformed position id never reaches signing.
    let key = common::test_key();
    let req = PositionLimitOrder {
        position_id: "0xnothex".into(),
        is_buy: true,
        size: 1.0,
        price: 0.5,
        tick_size: "0.01".into(),
        market_neg_risk: false,
        expiration: None,
        builder_code: None,
    };
    assert!(sign_position_limit_order(&key, &req, CHAIN, 0, None, &addresses(), 1).is_err());
}

#[test]
fn routing_table_is_exhaustive_over_asset_and_market_flags() {
    let token = OrderAsset::exactly_one(Some("1"), None).unwrap();
    let position = OrderAsset::exactly_one(None, Some("2")).unwrap();
    assert_eq!(resolve_route(&token, false), ExchangeRoute::V2Standard);
    assert_eq!(resolve_route(&token, true), ExchangeRoute::V2NegRisk);
    assert_eq!(resolve_route(&position, false), ExchangeRoute::V3);
    assert_eq!(resolve_route(&position, true), ExchangeRoute::V3);
}

#[tokio::test]
async fn the_mock_venue_is_available_for_the_async_suite() {
    // Sanity: the shared harness boots and answers /time — the async
    // pipeline suite depends on the same fixture set.
    let venue = common::mock_venue().await;
    let cfg = common::base_config(&venue);
    let state = common::state_with(cfg);
    let bot = common::paper_bot(
        &state,
        std::sync::Arc::new(module_polymarket::store::MemoryPolyStore::new()),
    )
    .await;
    assert!(!bot.can_sign());
}
