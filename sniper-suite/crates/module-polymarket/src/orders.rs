//! Order sizing, tick-size rounding and V2 order construction.
//!
//! Mirrors `py-clob-client`'s `OrderBuilder.get_order_amounts`: amounts are
//! rounded according to the market's tick size, then scaled to the 6-decimal
//! raw units both USDC and CTF outcome tokens use on Polygon. The result is a
//! [`OrderV2`] ready to be EIP-712 signed.

use k256::ecdsa::SigningKey;

use crate::eip712::{self, OrderV2, SIDE_BUY, SIDE_SELL};
use crate::error::{PolyError, PolyResult};

/// USDC and CTF outcome tokens are both 6 decimals on Polygon.
pub const TOKEN_DECIMALS: u32 = 6;
/// 10^6 as a f64, for scaling human amounts to raw units.
const SCALE: f64 = 1_000_000.0;

/// Per-tick rounding precision (decimal places), matching the Python client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoundConfig {
    /// Decimal places for the price.
    pub price: usize,
    /// Decimal places for the order size.
    pub size: usize,
    /// Decimal places for the computed raw amount.
    pub amount: usize,
}

impl RoundConfig {
    /// The rounding config for a tick size string ("0.1", "0.01", …).
    pub fn for_tick(tick: &str) -> PolyResult<Self> {
        Ok(match tick {
            "0.1" => RoundConfig {
                price: 1,
                size: 2,
                amount: 3,
            },
            "0.01" => RoundConfig {
                price: 2,
                size: 2,
                amount: 4,
            },
            "0.001" => RoundConfig {
                price: 3,
                size: 2,
                amount: 5,
            },
            "0.0001" => RoundConfig {
                price: 4,
                size: 2,
                amount: 6,
            },
            other => {
                return Err(PolyError::invalid(format!(
                    "unsupported tick size: {other}"
                )))
            }
        })
    }
}

/// Count the decimal places in a positive f64 (0 for integers).
fn decimal_places(x: f64) -> usize {
    if !x.is_finite() || x == 0.0 {
        return 0;
    }
    let s = format!("{x}");
    match s.split_once('.') {
        Some((_i, frac)) => frac.trim_end_matches('0').len(),
        None => 0,
    }
}

/// Round to `dp` decimal places, half away from zero.
fn round_normal(x: f64, dp: usize) -> f64 {
    let factor = 10f64.powi(dp as i32);
    (x * factor).round() / factor
}

/// Truncate (round toward zero) at `dp` decimal places.
fn round_down(x: f64, dp: usize) -> f64 {
    let factor = 10f64.powi(dp as i32);
    (x * factor).trunc() / factor
}

/// Round away from zero at `dp` decimal places.
fn round_up(x: f64, dp: usize) -> f64 {
    let factor = 10f64.powi(dp as i32);
    (x * factor).ceil() / factor
}

/// Scale a human amount to 6-decimal raw units.
fn to_token_decimals(x: f64) -> u128 {
    let scaled = x * SCALE;
    // Guard against tiny negative drift from float arithmetic.
    if scaled <= 0.0 {
        0
    } else {
        scaled.round() as u128
    }
}

/// The computed raw amounts for an order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OrderAmounts {
    /// `SIDE_BUY` or `SIDE_SELL`.
    pub side: u8,
    /// Amount the maker gives (USDC for a buy, tokens for a sell), raw 6dp.
    pub maker_amount: u128,
    /// Amount the maker receives (tokens for a buy, USDC for a sell), raw 6dp.
    pub taker_amount: u128,
}

/// Compute limit-order amounts for `size` outcome tokens at `price`.
///
/// `is_buy` selects the direction. Follows the Python client's rounding: the
/// size is truncated to the tick's size precision, the price is rounded to the
/// tick's price precision, and the product is nudged to the amount precision.
pub fn limit_order_amounts(is_buy: bool, size: f64, price: f64, cfg: RoundConfig) -> OrderAmounts {
    let raw_price = round_normal(price, cfg.price);
    if is_buy {
        let raw_taker = round_down(size, cfg.size);
        let mut raw_maker = raw_taker * raw_price;
        if decimal_places(raw_maker) > cfg.amount {
            raw_maker = round_up(raw_maker, cfg.amount + 4);
            if decimal_places(raw_maker) > cfg.amount {
                raw_maker = round_down(raw_maker, cfg.amount);
            }
        }
        OrderAmounts {
            side: SIDE_BUY,
            maker_amount: to_token_decimals(raw_maker),
            taker_amount: to_token_decimals(raw_taker),
        }
    } else {
        let raw_maker = round_down(size, cfg.size);
        let mut raw_taker = raw_maker * raw_price;
        if decimal_places(raw_taker) > cfg.amount {
            raw_taker = round_up(raw_taker, cfg.amount + 4);
            if decimal_places(raw_taker) > cfg.amount {
                raw_taker = round_down(raw_taker, cfg.amount);
            }
        }
        OrderAmounts {
            side: SIDE_SELL,
            maker_amount: to_token_decimals(raw_maker),
            taker_amount: to_token_decimals(raw_taker),
        }
    }
}

/// Parameters needed to assemble and sign an order.
pub struct OrderParams<'a> {
    /// The CTF ERC-1155 token id (decimal string).
    pub token_id: &'a str,
    /// Buy (true) or sell (false).
    pub is_buy: bool,
    /// Size in outcome tokens (human units).
    pub size: f64,
    /// Limit price in [0, 1] (human units).
    pub price: f64,
    /// Tick size string, e.g. "0.01".
    pub tick_size: &'a str,
    /// `true` for a neg-risk market (uses the neg-risk exchange as verifier).
    pub neg_risk: bool,
    /// Chain id (137 for Polygon).
    pub chain_id: u64,
    /// EIP-712 domain version ("2").
    pub domain_version: &'a str,
    /// The exchange contract that verifies the order.
    pub exchange_address: &'a str,
    /// The neg-risk exchange contract.
    pub neg_risk_exchange_address: &'a str,
    /// Signature type (0 EOA, 1 proxy, 2 safe, 3 deposit wallet).
    pub signature_type: u8,
    /// Address holding the funds (maker). Defaults to the signer when `None`.
    pub funder: Option<&'a str>,
    /// Order lifetime: `None`/0 for GTC, else a unix timestamp for GTD.
    pub expiration_timestamp: u64,
    /// Builder code (bytes32 hex) or `None` for zeros.
    pub builder_code: Option<&'a str>,
}

/// A built order plus the EIP-712 verifying contract used to sign it.
#[derive(Debug, Clone)]
pub struct SignedOrderBundle {
    /// The signed order fields.
    pub order: OrderV2,
    /// The `0x`-hex signature (r||s||v), possibly wrapped for type 3.
    pub signature: String,
    /// The exchange address that verifies this order.
    pub verifying_contract: String,
    /// Whether this is a neg-risk market.
    pub neg_risk: bool,
}

impl SignedOrderBundle {
    /// The CLOB order id this bundle will be assigned: the exchange's
    /// `getOrderHash(order)` equals `keccak256(abi.encode(Order))` — the
    /// EIP-712 struct hash — so it is derivable locally BEFORE (and without)
    /// the HTTP response. Reconciliation uses this to resolve submit-unknown
    /// outcomes (POST timed out: is the order resting/matched/absent?).
    pub fn derived_order_id(&self) -> PolyResult<String> {
        Ok(format!("0x{}", hex::encode(self.order.struct_hash()?)))
    }
}

/// Build and sign an order, returning the bundle the CLOB expects.
pub fn sign_order_bundle(key: &SigningKey, params: &OrderParams) -> PolyResult<SignedOrderBundle> {
    let signer_address = eip712::address_from_signing_key(key);
    let cfg = RoundConfig::for_tick(params.tick_size)?;
    let amounts = limit_order_amounts(params.is_buy, params.size, params.price, cfg);

    let verifying_contract = if params.neg_risk {
        params.neg_risk_exchange_address
    } else {
        params.exchange_address
    };

    let maker = match params.funder {
        Some(f) if !f.trim().is_empty() => f.to_string(),
        _ => signer_address.clone(),
    };

    // Deterministic salt (Prompt 2 §G/§Q): the CLOB order id is
    // keccak256(abi.encode(Order)), so deriving the salt purely from the
    // intent's semantic content makes a re-signed copy of the SAME intent —
    // after an ambiguous HTTP timeout, a crash/restart, or a strategy retry —
    // map to the SAME order id. The venue then deduplicates it instead of
    // resting a second live order (never double-trade the same intent).
    // GTD intents carry their expiration in `timestamp`, so each new expiry
    // window naturally produces a fresh order id; deliberately re-ordering an
    // identical GTC intent requires changing size/price.
    let mut salt_pre: Vec<u8> = Vec::with_capacity(256);
    salt_pre.extend_from_slice(b"polymarket-salt-v1|");
    salt_pre.extend_from_slice(maker.as_bytes());
    salt_pre.push(b'|');
    salt_pre.extend_from_slice(signer_address.as_bytes());
    salt_pre.extend_from_slice(
        format!(
            "|{}|{}|{}|{}|{}|{}|{}",
            params.token_id,
            amounts.maker_amount,
            amounts.taker_amount,
            amounts.side,
            params.signature_type,
            params.expiration_timestamp,
            params.builder_code.unwrap_or("")
        )
        .as_bytes(),
    );
    let salt_hash = eip712::keccak256(&salt_pre);
    let salt: u128 = u128::from_be_bytes(
        salt_hash[..16]
            .try_into()
            .expect("keccak output is 32 bytes"),
    ) | 1; // never zero
    let order = OrderV2 {
        salt: salt.to_string(),
        maker,
        signer: signer_address.clone(),
        token_id: params.token_id.to_string(),
        maker_amount: amounts.maker_amount.to_string(),
        taker_amount: amounts.taker_amount.to_string(),
        side: amounts.side,
        signature_type: params.signature_type,
        timestamp: params.expiration_timestamp.to_string(),
        metadata: String::new(),
        builder: params.builder_code.unwrap_or("").to_string(),
    };

    let digest = eip712::order_digest(
        params.chain_id,
        params.domain_version,
        verifying_contract,
        &order,
    )?;
    let raw_sig = eip712::sign_digest(key, &digest)?;

    // Type 3 (deposit wallet / EIP-1271) wraps the inner signature.
    let signature = if params.signature_type == 3 {
        let wrapped = eip712::wrap_deposit_wallet_signature(
            &raw_sig,
            params.chain_id,
            params.domain_version,
            verifying_contract,
            &order,
        )?;
        format!("0x{}", hex::encode(wrapped))
    } else {
        format!("0x{}", hex::encode(raw_sig))
    };

    Ok(SignedOrderBundle {
        order,
        signature,
        verifying_contract: verifying_contract.to_string(),
        neg_risk: params.neg_risk,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_rounding_configs() {
        assert_eq!(RoundConfig::for_tick("0.1").unwrap().amount, 3);
        assert_eq!(RoundConfig::for_tick("0.01").unwrap().price, 2);
        assert_eq!(RoundConfig::for_tick("0.0001").unwrap().amount, 6);
        assert!(RoundConfig::for_tick("0.5").is_err());
    }

    #[test]
    fn buy_amounts_scale_to_six_decimals() {
        // Buy 10 tokens at 0.50 with a 0.01 tick: USDC in = 5.0, tokens out = 10.
        let cfg = RoundConfig::for_tick("0.01").unwrap();
        let a = limit_order_amounts(true, 10.0, 0.50, cfg);
        assert_eq!(a.side, SIDE_BUY);
        assert_eq!(a.maker_amount, 5_000_000); // 5 USDC
        assert_eq!(a.taker_amount, 10_000_000); // 10 tokens
    }

    #[test]
    fn sell_amounts_scale_to_six_decimals() {
        // Sell 10 tokens at 0.60: tokens in = 10, USDC out = 6.
        let cfg = RoundConfig::for_tick("0.01").unwrap();
        let a = limit_order_amounts(false, 10.0, 0.60, cfg);
        assert_eq!(a.side, SIDE_SELL);
        assert_eq!(a.maker_amount, 10_000_000); // 10 tokens
        assert_eq!(a.taker_amount, 6_000_000); // 6 USDC
    }

    #[test]
    fn price_is_rounded_to_the_tick() {
        // 0.01 tick => price rounded to 2 dp: 0.567 -> 0.57.
        let cfg = RoundConfig::for_tick("0.01").unwrap();
        let a = limit_order_amounts(true, 1.0, 0.567, cfg);
        // 1 token * 0.57 = 0.57 USDC.
        assert_eq!(a.taker_amount, 1_000_000);
        assert_eq!(a.maker_amount, 570_000);
    }

    #[test]
    fn sign_bundle_recovers_and_uses_neg_risk_contract() {
        let key_bytes =
            hex::decode("4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318")
                .unwrap();
        let key = SigningKey::from_slice(&key_bytes).unwrap();
        let params = OrderParams {
            token_id: "123456789",
            is_buy: true,
            size: 5.0,
            price: 0.4,
            tick_size: "0.01",
            neg_risk: true,
            chain_id: 137,
            domain_version: "2",
            exchange_address: "0xE111180000d2663C0091e4f400237545B87B996B",
            neg_risk_exchange_address: "0xe2222d279d744050d28e00520010520000310F59",
            signature_type: 0,
            funder: None,
            expiration_timestamp: 0,
            builder_code: None,
        };
        let bundle = sign_order_bundle(&key, &params).unwrap();
        // neg_risk => the neg-risk exchange verifies.
        assert_eq!(
            bundle.verifying_contract,
            "0xe2222d279d744050d28e00520010520000310F59"
        );
        assert!(bundle.signature.starts_with("0x"));
        // EOA signature is 65 bytes => 130 hex chars + "0x".
        assert_eq!(bundle.signature.len(), 2 + 130);
        assert_eq!(bundle.order.side, SIDE_BUY);
        assert_eq!(bundle.order.maker_amount, "2000000"); // 5 * 0.4 = 2 USDC
        assert_eq!(bundle.order.taker_amount, "5000000"); // 5 tokens
    }

    fn test_params<'a>(size: f64, expiration: u64) -> OrderParams<'a> {
        OrderParams {
            token_id: "12345678901234567890",
            is_buy: true,
            size,
            price: 0.5,
            tick_size: "0.01",
            neg_risk: false,
            chain_id: 137,
            domain_version: "2",
            exchange_address: "0xE111180000d2663C0091e4f400237545B87B996B",
            neg_risk_exchange_address: "0xe2222d279d744050d28e00520010520000310F59",
            signature_type: 0,
            funder: None,
            expiration_timestamp: expiration,
            builder_code: None,
        }
    }

    #[test]
    fn derived_order_id_is_deterministic_per_intent() {
        // §G/§Q: re-signing the SAME intent (e.g. after an ambiguous HTTP
        // timeout or a restart) must produce the SAME CLOB order id so the
        // venue deduplicates instead of resting a second live order.
        let key = SigningKey::from_slice(&[7u8; 32]).unwrap();
        let a = sign_order_bundle(&key, &test_params(10.0, 0)).unwrap();
        let b = sign_order_bundle(&key, &test_params(10.0, 0)).unwrap();
        assert_eq!(a.derived_order_id().unwrap(), b.derived_order_id().unwrap());
        // Different size → different intent → different id.
        let c = sign_order_bundle(&key, &test_params(11.0, 0)).unwrap();
        assert_ne!(a.derived_order_id().unwrap(), c.derived_order_id().unwrap());
        // GTD: a new expiration window is a new intent.
        let d = sign_order_bundle(&key, &test_params(10.0, 1_900_000_000)).unwrap();
        assert_ne!(a.derived_order_id().unwrap(), d.derived_order_id().unwrap());
        // Another signer → different id (identity is part of the salt).
        let key2 = SigningKey::from_slice(&[9u8; 32]).unwrap();
        let e = sign_order_bundle(&key2, &test_params(10.0, 0)).unwrap();
        assert_ne!(a.derived_order_id().unwrap(), e.derived_order_id().unwrap());
        // Format: 0x + 64 hex chars (keccak256).
        let id = a.derived_order_id().unwrap();
        assert!(id.starts_with("0x") && id.len() == 66, "{id}");
    }
}
