//! EIP-712 typed-data signing for Polymarket CLOB **V2** orders.
//!
//! Polymarket's 2026 CLOB V2 cutover changed the `Order` struct: it dropped
//! `taker`, `expiration`, `nonce` and `feeRateBps` and added `timestamp`,
//! `metadata` and `builder`. The domain version moved to `"2"`. Signing a V1
//! struct against the V2 exchange is rejected with `order_version_mismatch`, so
//! the exact field list below matters.
//!
//! V2 order type (11 fields):
//! ```text
//! Order(uint256 salt,address maker,address signer,uint256 tokenId,
//!       uint256 makerAmount,uint256 takerAmount,uint8 side,uint8 signatureType,
//!       uint256 timestamp,bytes32 metadata,bytes32 builder)
//! ```
//! Domain: `EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)`
//! with `name = "Polymarket CTF Exchange"`.
//!
//! All hashing is keccak256 (NOT SHA3-256). Amounts and `tokenId` are uint256;
//! we accept them as decimal strings and encode big-endian into 32 bytes.

use k256::ecdsa::{RecoveryId, Signature, SigningKey, VerifyingKey};
use num_bigint::BigUint;
use once_cell::sync::Lazy;
use tiny_keccak::{Hasher, Keccak};

use crate::error::{PolyError, PolyResult};

/// `keccak256("EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)")`
pub const DOMAIN_TYPE_STR: &str =
    "EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)";

/// `keccak256("Order(...)")` for the V2 11-field struct.
pub const ORDER_TYPE_STR: &str = "Order(uint256 salt,address maker,address signer,uint256 tokenId,uint256 makerAmount,uint256 takerAmount,uint8 side,uint8 signatureType,uint256 timestamp,bytes32 metadata,bytes32 builder)";

/// The EIP-712 domain name for the CTF exchange.
pub const DOMAIN_NAME: &str = "Polymarket CTF Exchange";

/// Cached `keccak256(ORDER_TYPE_STR)` — the V2 `Order` struct typehash used
/// as the first word of the struct hash.
pub static ORDER_TYPEHASH: Lazy<[u8; 32]> = Lazy::new(|| keccak256(ORDER_TYPE_STR.as_bytes()));
/// Cached `keccak256(DOMAIN_TYPE_STR)` — the EIP-712 domain typehash.
pub static DOMAIN_TYPEHASH: Lazy<[u8; 32]> = Lazy::new(|| keccak256(DOMAIN_TYPE_STR.as_bytes()));

/// Order side (matches the on-chain `Side` enum).
pub const SIDE_BUY: u8 = 0;
/// Sell side (`Side.SELL`).
pub const SIDE_SELL: u8 = 1;

/// keccak256 over `data`.
pub fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak::v256();
    hasher.update(data);
    let mut out = [0u8; 32];
    hasher.finalize(&mut out);
    out
}

/// Encode a decimal-string uint256 as 32 big-endian bytes.
pub fn u256_word(decimal: &str) -> PolyResult<[u8; 32]> {
    let trimmed = decimal.trim();
    if trimmed.is_empty() {
        return Err(PolyError::invalid("empty uint256"));
    }
    // Allow 0x-prefixed hex as well as plain decimal.
    let value = if let Some(hex_digits) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        BigUint::parse_bytes(hex_digits.as_bytes(), 16)
            .ok_or_else(|| PolyError::invalid(format!("bad hex uint256: {trimmed}")))?
    } else {
        BigUint::parse_bytes(trimmed.as_bytes(), 10)
            .ok_or_else(|| PolyError::invalid(format!("bad decimal uint256: {trimmed}")))?
    };
    let bytes = value.to_bytes_be();
    if bytes.len() > 32 {
        return Err(PolyError::invalid("uint256 overflow"));
    }
    let mut word = [0u8; 32];
    word[32 - bytes.len()..].copy_from_slice(&bytes);
    Ok(word)
}

/// Encode a `0x`-prefixed 20-byte address as a left-padded 32-byte word.
pub fn address_word(addr: &str) -> PolyResult<[u8; 32]> {
    let bytes = parse_address(addr)?;
    let mut word = [0u8; 32];
    word[12..].copy_from_slice(&bytes);
    Ok(word)
}

/// Parse a `0x`-prefixed address (case-insensitive) into 20 bytes.
pub fn parse_address(addr: &str) -> PolyResult<[u8; 20]> {
    let hexstr = addr
        .strip_prefix("0x")
        .or_else(|| addr.strip_prefix("0X"))
        .unwrap_or(addr);
    if hexstr.len() != 40 {
        return Err(PolyError::invalid(format!("address not 20 bytes: {addr}")));
    }
    let bytes = hex::decode(hexstr).map_err(|e| PolyError::invalid(format!("address hex: {e}")))?;
    let mut out = [0u8; 20];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Parse a `0x`-prefixed 32-byte word (metadata / builder). Zeros if empty.
pub fn bytes32_word(hexstr: &str) -> PolyResult<[u8; 32]> {
    if hexstr.trim().is_empty() {
        return Ok([0u8; 32]);
    }
    let h = hexstr
        .strip_prefix("0x")
        .or_else(|| hexstr.strip_prefix("0X"))
        .unwrap_or(hexstr);
    let bytes = hex::decode(h).map_err(|e| PolyError::invalid(format!("bytes32 hex: {e}")))?;
    if bytes.len() != 32 {
        return Err(PolyError::invalid(format!(
            "bytes32 must be 32 bytes, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// A small uint8 encoded as a 32-byte word (left-padded), per `abi.encode`.
fn uint8_word(v: u8) -> [u8; 32] {
    let mut word = [0u8; 32];
    word[31] = v;
    word
}

/// The V2 order payload, prior to signing.
///
/// `salt`, `token_id`, `maker_amount`, `taker_amount` and `timestamp` are
/// decimal (or `0x`-hex) uint256 strings so no precision is lost on the huge
/// CTF token ids. `metadata`/`builder` are `0x`-hex bytes32 (zeros when empty).
#[derive(Debug, Clone)]
pub struct OrderV2 {
    /// Unique per-order nonce (decimal uint256 string) preventing replay.
    pub salt: String,
    /// `0x` address of the maker (funder: proxy/safe/deposit wallet or EOA).
    pub maker: String,
    /// `0x` address whose signature authorizes the order (== maker for EOA).
    pub signer: String,
    /// CTF outcome token id being traded (decimal uint256 string).
    pub token_id: String,
    /// What the maker SENDS, raw 6-dec string: collateral for a buy,
    /// outcome tokens for a sell.
    pub maker_amount: String,
    /// What the maker RECEIVES, raw 6-dec string: outcome tokens for a buy,
    /// collateral for a sell.
    pub taker_amount: String,
    /// 0 = buy, 1 = sell ([`SIDE_BUY`] / [`SIDE_SELL`]).
    pub side: u8,
    /// 0 EOA, 1 Polymarket proxy, 2 Gnosis safe, 3 deposit wallet.
    pub signature_type: u8,
    /// Unix-seconds string. This implementation carries the GTD expiry here
    /// (`orders.rs` maps `expiration_timestamp` into this field) and uses
    /// `"0"` for GTC/FOK/FAK orders.
    pub timestamp: String,
    /// `0x`-hex bytes32 order metadata (zeros/empty when unused).
    pub metadata: String,
    /// `0x`-hex bytes32 builder code for fee attribution (zeros when none).
    pub builder: String,
}

impl OrderV2 {
    /// The `abi.encode`d struct hash input (typehash followed by each field).
    pub fn encode(&self) -> PolyResult<Vec<u8>> {
        let mut buf = Vec::with_capacity(12 * 32);
        buf.extend_from_slice(&*ORDER_TYPEHASH);
        buf.extend_from_slice(&u256_word(&self.salt)?);
        buf.extend_from_slice(&address_word(&self.maker)?);
        buf.extend_from_slice(&address_word(&self.signer)?);
        buf.extend_from_slice(&u256_word(&self.token_id)?);
        buf.extend_from_slice(&u256_word(&self.maker_amount)?);
        buf.extend_from_slice(&u256_word(&self.taker_amount)?);
        buf.extend_from_slice(&uint8_word(self.side));
        buf.extend_from_slice(&uint8_word(self.signature_type));
        buf.extend_from_slice(&u256_word(&self.timestamp)?);
        buf.extend_from_slice(&bytes32_word(&self.metadata)?);
        buf.extend_from_slice(&bytes32_word(&self.builder)?);
        Ok(buf)
    }

    /// `keccak256(abi.encode(Order(...)))`.
    pub fn struct_hash(&self) -> PolyResult<[u8; 32]> {
        Ok(keccak256(&self.encode()?))
    }
}

/// Compute the EIP-712 domain separator.
pub fn domain_separator(
    chain_id: u64,
    version: &str,
    verifying_contract: &str,
) -> PolyResult<[u8; 32]> {
    let mut buf = Vec::with_capacity(5 * 32);
    buf.extend_from_slice(&*DOMAIN_TYPEHASH);
    buf.extend_from_slice(&keccak256(DOMAIN_NAME.as_bytes()));
    buf.extend_from_slice(&keccak256(version.as_bytes()));
    let mut chain_word = [0u8; 32];
    chain_word[24..].copy_from_slice(&chain_id.to_be_bytes());
    buf.extend_from_slice(&chain_word);
    buf.extend_from_slice(&address_word(verifying_contract)?);
    Ok(keccak256(&buf))
}

/// The EIP-712 signing digest: `keccak256(0x19 0x01 || domainSep || structHash)`.
pub fn order_digest(
    chain_id: u64,
    version: &str,
    verifying_contract: &str,
    order: &OrderV2,
) -> PolyResult<[u8; 32]> {
    let domain = domain_separator(chain_id, version, verifying_contract)?;
    let struct_hash = order.struct_hash()?;
    let mut buf = Vec::with_capacity(2 + 64);
    buf.push(0x19);
    buf.push(0x01);
    buf.extend_from_slice(&domain);
    buf.extend_from_slice(&struct_hash);
    Ok(keccak256(&buf))
}

/// Derive the 20-byte Ethereum address for a signing key (lowercase, `0x`).
pub fn address_from_signing_key(key: &SigningKey) -> String {
    let verifying = VerifyingKey::from(key);
    let point = verifying.to_encoded_point(false); // uncompressed: 0x04 || X || Y
    let hash = keccak256(&point.as_bytes()[1..]); // drop the 0x04 prefix
    format!("0x{}", hex::encode(&hash[12..]))
}

/// Sign a 32-byte digest, returning the 65-byte `r || s || v` signature
/// (`v` in {27, 28}), with `s` normalised to the low half of the curve order.
pub fn sign_digest(key: &SigningKey, digest: &[u8; 32]) -> PolyResult<[u8; 65]> {
    let (sig, recid): (Signature, RecoveryId) = key
        .sign_prehash_recoverable(digest)
        .map_err(|e| PolyError::invalid(format!("signing failed: {e}")))?;
    let mut out = [0u8; 65];
    out[..64].copy_from_slice(&sig.to_bytes());
    out[64] = 27 + recid.to_byte();
    Ok(out)
}

/// Convenience: build the digest for an order and sign it, returning `0x`-hex.
pub fn sign_order(
    key: &SigningKey,
    chain_id: u64,
    version: &str,
    verifying_contract: &str,
    order: &OrderV2,
) -> PolyResult<String> {
    let digest = order_digest(chain_id, version, verifying_contract, order)?;
    let sig = sign_digest(key, &digest)?;
    Ok(format!("0x{}", hex::encode(sig)))
}

/// EIP-55 mixed-case checksum encoding of a `0x` address (used for display and
/// for a few Polymarket endpoints that compare checksummed strings).
pub fn to_checksum_address(addr: &str) -> PolyResult<String> {
    let bytes = parse_address(addr)?;
    let lower = hex::encode(bytes);
    let hash = keccak256(lower.as_bytes());
    let mut out = String::with_capacity(42);
    out.push_str("0x");
    for (i, ch) in lower.chars().enumerate() {
        if ch.is_ascii_digit() {
            out.push(ch);
        } else {
            // Each hash byte covers two hex chars; use the high nibble for the
            // first char, low nibble for the second.
            let nibble = if i % 2 == 0 {
                hash[i / 2] >> 4
            } else {
                hash[i / 2] & 0x0f
            };
            if nibble >= 8 {
                out.push(ch.to_ascii_uppercase());
            } else {
                out.push(ch);
            }
        }
    }
    Ok(out)
}

/// Wrap an inner order signature for `signature_type = 3` (deposit wallet /
/// EIP-1271), per the Polymarket docs:
/// `inner || appDomainSeparator || contentsHash || ORDER_TYPE || len(ORDER_TYPE)`
/// where `len` is a 2-byte big-endian byte length of the ORDER_TYPE string.
pub fn wrap_deposit_wallet_signature(
    inner_signature: &[u8],
    chain_id: u64,
    version: &str,
    verifying_contract: &str,
    order: &OrderV2,
) -> PolyResult<Vec<u8>> {
    let app_domain = domain_separator(chain_id, version, verifying_contract)?;
    let contents_hash = order.struct_hash()?;
    let order_type = ORDER_TYPE_STR.as_bytes();
    let mut out = Vec::new();
    out.extend_from_slice(inner_signature);
    out.extend_from_slice(&app_domain);
    out.extend_from_slice(&contents_hash);
    out.extend_from_slice(order_type);
    out.extend_from_slice(&(order_type.len() as u16).to_be_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keccak_of_empty_is_the_known_constant() {
        let h = keccak256(b"");
        assert_eq!(
            hex::encode(h),
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
    }

    #[test]
    fn keccak_differs_from_sha3() {
        // "abc" under keccak256 vs SHA3-256 are different; guard against a swap.
        let h = keccak256(b"abc");
        assert_eq!(
            hex::encode(h),
            "4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45"
        );
    }

    #[test]
    fn u256_word_pads_big_endian() {
        let w = u256_word("1").unwrap();
        assert_eq!(w[31], 1);
        assert_eq!(w[0], 0);
        let big = u256_word(
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
        )
        .unwrap();
        assert_eq!(big, [0xff; 32]);
        assert!(u256_word(
            "115792089237316195423570985008687907853269984665640564039457584007913129639936"
        )
        .is_err());
    }

    #[test]
    fn u256_word_accepts_hex() {
        assert_eq!(u256_word("0xff").unwrap(), u256_word("255").unwrap());
    }

    #[test]
    fn address_word_left_pads() {
        let w = address_word("0x0000000000000000000000000000000000000001").unwrap();
        assert_eq!(w[31], 1);
        assert_eq!(&w[..12], &[0u8; 12]);
    }

    #[test]
    fn order_encode_is_12_words() {
        let order = OrderV2 {
            salt: "479249096354".into(),
            maker: "0x0000000000000000000000000000000000000001".into(),
            signer: "0x0000000000000000000000000000000000000002".into(),
            token_id: "123".into(),
            maker_amount: "5200000".into(),
            taker_amount: "10000000".into(),
            side: SIDE_BUY,
            signature_type: 0,
            timestamp: "0".into(),
            metadata: "".into(),
            builder: "".into(),
        };
        assert_eq!(order.encode().unwrap().len(), 12 * 32);
    }

    #[test]
    fn sign_and_recover_roundtrip() {
        // A fixed 32-byte private key.
        let key_bytes =
            hex::decode("4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318")
                .unwrap();
        let key = SigningKey::from_slice(&key_bytes).unwrap();
        let signer_addr = address_from_signing_key(&key);

        let order = OrderV2 {
            salt: "1".into(),
            maker: signer_addr.clone(),
            signer: signer_addr.clone(),
            token_id: "999".into(),
            maker_amount: "1000000".into(),
            taker_amount: "2000000".into(),
            side: SIDE_BUY,
            signature_type: 0,
            timestamp: "0".into(),
            metadata: "".into(),
            builder: "".into(),
        };
        let digest = order_digest(
            137,
            "2",
            "0xE111180000d2663C0091e4f400237545B87B996B",
            &order,
        )
        .unwrap();
        let sig = sign_digest(&key, &digest).unwrap();
        assert_eq!(sig.len(), 65);
        assert!(sig[64] == 27 || sig[64] == 28);

        // Recover the signer from the signature and confirm it matches.
        let recid = RecoveryId::from_byte(sig[64] - 27).unwrap();
        let signature = Signature::from_slice(&sig[..64]).unwrap();
        let recovered = VerifyingKey::recover_from_prehash(&digest, &signature, recid).unwrap();
        let point = recovered.to_encoded_point(false);
        let hash = keccak256(&point.as_bytes()[1..]);
        let recovered_addr = format!("0x{}", hex::encode(&hash[12..]));
        assert_eq!(recovered_addr, signer_addr);
    }

    #[test]
    fn checksum_address_matches_eip55_vector() {
        // Known EIP-55 vector.
        let out = to_checksum_address("0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed").unwrap();
        assert_eq!(out, "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed");
    }

    #[test]
    fn domain_separator_is_deterministic() {
        let a = domain_separator(137, "2", "0xE111180000d2663C0091e4f400237545B87B996B").unwrap();
        let b = domain_separator(137, "2", "0xE111180000d2663C0091e4f400237545B87B996B").unwrap();
        assert_eq!(a, b);
        // A different version yields a different separator.
        let c = domain_separator(137, "1", "0xE111180000d2663C0091e4f400237545B87B996B").unwrap();
        assert_ne!(a, c);
    }

    #[test]
    fn wrap_deposit_wallet_appends_expected_tail() {
        let order = OrderV2 {
            salt: "1".into(),
            maker: "0x0000000000000000000000000000000000000001".into(),
            signer: "0x0000000000000000000000000000000000000002".into(),
            token_id: "5".into(),
            maker_amount: "1".into(),
            taker_amount: "2".into(),
            side: SIDE_BUY,
            signature_type: 3,
            timestamp: "0".into(),
            metadata: "".into(),
            builder: "".into(),
        };
        let inner = vec![0xAA; 65];
        let wrapped = wrap_deposit_wallet_signature(
            &inner,
            137,
            "2",
            "0xE111180000d2663C0091e4f400237545B87B996B",
            &order,
        )
        .unwrap();
        // 65 inner + 32 domain + 32 contents + ORDER_TYPE + 2 length bytes.
        assert_eq!(wrapped.len(), 65 + 32 + 32 + ORDER_TYPE_STR.len() + 2);
        // The final two bytes encode the ORDER_TYPE length.
        let len = u16::from_be_bytes([wrapped[wrapped.len() - 2], wrapped[wrapped.len() - 1]]);
        assert_eq!(len as usize, ORDER_TYPE_STR.len());
    }
}
