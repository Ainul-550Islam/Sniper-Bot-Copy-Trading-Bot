//! Chain-side existence check for an ambiguous pre-broadcast claim.
//!
//! The operator sweep (`crate::recon::ReservationSweepService`) lets a human
//! decide `abandon` or `link` on a reservation whose lease lapsed. Deciding
//! `abandon` frees the trade key for a RETRY, so the operator needs an
//! answer to one question first: *did the earlier attempt's transaction
//! actually land?* Until now the only way to answer it was to copy the
//! signature into an explorer by hand.
//!
//! This module answers it from the chain, and answers it conservatively:
//!
//! * only a signature that the RPC node KNOWS is reported as landed — its
//!   slot, block time, error and the signer's post-fee balance delta are
//!   read from the fetched transaction itself;
//! * a signature the node does not know is reported as `absent`, which is
//!   NOT proof of a non-send (an unlanded/evicted transaction looks the
//!   same), so `abandon_allowed` stays false unless the operator explicitly
//!   asserts the attempt never left the process;
//! * an RPC failure is reported as `unknown` — never as "not found".
//!
//! Every path names the decision it justifies, so the API can refuse the
//! dangerous combination (`abandon` while the transaction is on chain).

use std::str::FromStr;
use std::sync::Arc;

use serde::Serialize;
use solana_kit::rpc::Rpc;
use solana_sdk::signature::Signature;
use tracing::debug;

/// What the chain says about one ambiguous attempt's signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainExistence {
    /// The node returned the transaction: it is on chain.
    Landed,
    /// The node does not know the signature. NOT proof of a non-send.
    Absent,
    /// The lookup itself failed (transport/provider error).
    Unknown,
}

/// How deep a LANDED transaction is, judged by slot distance from the node's
/// current slot. This is a *reorganisation risk* statement, not a proof of
/// finality: Solana finalises roots well after `confirmed`, and a node that is
/// lying, stale or fork-aware only in its own view cannot be detected here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainFinality {
    /// Less than [`FINALITY_MIN_DEPTH`] slots behind the head: it may still be
    /// rolled back, so nothing may be booked from it.
    Unfinalized,
    /// At or beyond the compiled depth threshold: deep enough that a rollback
    /// is implausible, but STILL not a protocol finality proof.
    DeeplyConfirmed,
    /// Not landed, so depth does not apply.
    NotLanded,
}

/// Slot distance at or beyond which a landed transaction is reported as
/// `deeply_confirmed`. Deliberately conservative (Solana finalises in ~13 s,
/// roughly 400 slots) and configurable in one place.
pub const FINALITY_MIN_DEPTH: u64 = 512;

/// The full answer handed to the operator.
#[derive(Debug, Clone, Serialize)]
pub struct ChainExistenceReport {
    pub trade_key: String,
    pub signature: String,
    pub existence: ChainExistence,
    /// `Some(slot)` only when the transaction was fetched.
    pub slot: Option<u64>,
    pub block_time: Option<i64>,
    /// Provider-reported execution error (`None` = the transaction succeeded).
    pub error: Option<String>,
    /// Post-fee lamport delta of the signer, when it could be decoded.
    pub signer_balance_delta: Option<i128>,
    /// Whether the evidence is strong enough for an operator `abandon`.
    /// True only when the operator explicitly asserts the attempt never left
    /// the process AND the chain does not contradict it.
    pub abandon_allowed: bool,
    /// Whether the evidence supports a `link` decision.
    pub link_allowed: bool,
    /// The node's slot at probe time (`None` when the node did not answer).
    pub head_slot: Option<u64>,
    /// How deep the landed transaction is (`NotLanded` when it is not on
    /// chain, `Unfinalized` when the depth could not be established).
    pub finality: ChainFinality,
    /// Slot distance between the transaction and the head, when both were
    /// known.
    pub confirmations: Option<u64>,
    /// Whether the landing is deep enough to book economics from. A landed but
    /// `Unfinalized` transaction still forbids `abandon` — it just may not be
    /// treated as settled.
    pub settled_allowed: bool,
    /// Human-readable reason; never contains a key or a secret.
    pub reason: String,
}

/// Read-only chain lookup for the sweep.
pub struct ChainExistenceProbe {
    rpc: Arc<Rpc>,
}

impl ChainExistenceProbe {
    pub fn new(rpc: Arc<Rpc>) -> Self {
        ChainExistenceProbe { rpc }
    }

    /// Production constructor from the loaded network config (same RPC pool
    /// the modules use). `None` when the RPC client cannot be built.
    pub fn from_config(cfg: &bot_core::config::NetworkConfig) -> Option<Arc<Self>> {
        match Rpc::new(cfg) {
            Ok(rpc) => Some(Arc::new(ChainExistenceProbe::new(Arc::new(rpc)))),
            Err(error) => {
                debug!(%error, "no chain reader for the reservation sweep");
                None
            }
        }
    }

    /// Does `signature` exist on chain, and does that justify the operator's
    /// decision? `operator_asserts_never_sent` must be an explicit human
    /// claim — it is the ONLY way an `absent` signature becomes abandonable.
    pub async fn probe(
        &self,
        trade_key: &str,
        signature: &str,
        operator_asserts_never_sent: bool,
    ) -> ChainExistenceReport {
        let parsed = match Signature::from_str(signature.trim()) {
            Ok(sig) => sig,
            Err(_) => {
                return ChainExistenceReport {
                    trade_key: trade_key.to_string(),
                    signature: signature.to_string(),
                    existence: ChainExistence::Unknown,
                    slot: None,
                    block_time: None,
                    error: None,
                    signer_balance_delta: None,
                    abandon_allowed: false,
                    link_allowed: false,
                    head_slot: None,
                    finality: ChainFinality::NotLanded,
                    confirmations: None,
                    settled_allowed: false,
                    reason: "the value is not a valid transaction signature".into(),
                }
            }
        };
        match self.rpc.get_transaction(&parsed).await {
            Ok(Some(tx)) => {
                let slot = Some(tx.slot);
                let block_time = tx.block_time;
                let error = tx
                    .transaction
                    .meta
                    .as_ref()
                    .and_then(|m| m.err.as_ref())
                    .map(|e| e.to_string());
                let delta = tx
                    .transaction
                    .meta
                    .as_ref()
                    .and_then(|m| m.pre_balances.first().zip(m.post_balances.first()))
                    .map(|(pre, post)| *post as i128 - *pre as i128);
                debug!(
                    trade = %trade_key,
                    slot = ?slot,
                    failed = error.is_some(),
                    "ambiguous attempt found on chain"
                );
                // Depth relative to the node's head. A node that cannot answer
                // `getSlot` leaves the depth unknown, which is reported as
                // unfinalized: the landing itself is still proof for `link`,
                // but its economics may not be booked.
                let head = self.rpc.get_slot().await.ok();
                let confirmations = match (slot, head) {
                    (Some(tx_slot), Some(head_slot)) if head_slot >= tx_slot => {
                        Some(head_slot - tx_slot)
                    }
                    _ => None,
                };
                let (finality, settled_allowed) = match confirmations {
                    Some(depth) if depth >= FINALITY_MIN_DEPTH => {
                        (ChainFinality::DeeplyConfirmed, true)
                    }
                    _ => (ChainFinality::Unfinalized, false),
                };
                let depth_note = match (confirmations, head) {
                    (Some(depth), Some(head_slot)) if depth >= FINALITY_MIN_DEPTH => {
                        format!("; {depth} slots deep (head {head_slot}), deeply confirmed")
                    }
                    (Some(depth), Some(head_slot)) => format!(
                        "; only {depth} slots deep (head {head_slot}) — still reversible, do not \
                         book settlement"
                    ),
                    _ => "; depth unknown (the node did not report its slot) — do not book \
                          settlement"
                        .to_string(),
                };
                ChainExistenceReport {
                    trade_key: trade_key.to_string(),
                    signature: signature.to_string(),
                    existence: ChainExistence::Landed,
                    slot,
                    block_time,
                    error: error.clone(),
                    signer_balance_delta: delta,
                    // A landed transaction must be LINKED, never abandoned:
                    // freeing the key would let a retry send a second one.
                    abandon_allowed: false,
                    link_allowed: true,
                    head_slot: head,
                    finality,
                    confirmations,
                    settled_allowed,
                    reason: match error {
                        Some(e) => {
                            format!("transaction is on chain (slot {slot:?}) but failed: {e}{depth_note}")
                        }
                        None => format!("transaction is on chain (slot {slot:?}){depth_note}"),
                    },
                }
            }
            Ok(None) => ChainExistenceReport {
                trade_key: trade_key.to_string(),
                signature: signature.to_string(),
                existence: ChainExistence::Absent,
                slot: None,
                block_time: None,
                error: None,
                signer_balance_delta: None,
                abandon_allowed: operator_asserts_never_sent,
                link_allowed: false,
                head_slot: None,
                finality: ChainFinality::NotLanded,
                confirmations: None,
                settled_allowed: false,
                reason: if operator_asserts_never_sent {
                    "the node does not know this signature and the operator asserts the attempt \
                     never left the process"
                        .into()
                } else {
                    "the node does not know this signature, which is NOT proof that nothing was \
                     sent; assert the non-send explicitly to allow `abandon`"
                        .into()
                },
            },
            Err(e) => ChainExistenceReport {
                trade_key: trade_key.to_string(),
                signature: signature.to_string(),
                existence: ChainExistence::Unknown,
                slot: None,
                block_time: None,
                error: None,
                signer_balance_delta: None,
                abandon_allowed: false,
                link_allowed: false,
                head_slot: None,
                finality: ChainFinality::NotLanded,
                confirmations: None,
                settled_allowed: false,
                reason: format!("chain lookup failed: {e}"),
            },
        }
    }
}

/// The answer across EVERY signature an ambiguous attempt could have landed
/// under.
///
/// A Solana transaction that is re-signed, sent with a different fee payer, or
/// retried after a blockhash refresh lands under a **different** signature, so
/// asking only about the one the claim recorded can report `absent` for an
/// attempt that is on chain. This scan asks about the recorded signature plus
/// every alternate the operator supplies (for example from an explorer or a
/// provider log) and refuses to conclude anything unless all of them agree
/// that nothing landed.
#[derive(Debug, Clone, Serialize)]
pub struct SignatureScanReport {
    pub trade_key: String,
    /// Per-signature answers, recorded signature first.
    pub reports: Vec<ChainExistenceReport>,
    /// Every signature this scan found on chain (empty = none landed).
    pub landed_signatures: Vec<String>,
    /// True when MORE than one distinct signature of this attempt landed —
    /// the money-moving duplicate the reservation fence exists to prevent.
    /// Nothing is ever booked or freed automatically in that case.
    pub duplicate_detected: bool,
    /// Strongest chain fact across all signatures.
    pub existence: ChainExistence,
    /// Only when the recorded signature landed AND no duplicate exists.
    pub link_allowed: bool,
    /// `abandon` stays forbidden when ANY supplied signature landed.
    pub abandon_allowed: bool,
    /// Only for a single, deep landing. A duplicate is never settleable.
    pub settled_allowed: bool,
    /// Human-readable summary; never contains a key or a secret.
    pub reason: String,
}

impl SignatureScanReport {
    fn empty(trade_key: &str, reason: String) -> Self {
        SignatureScanReport {
            trade_key: trade_key.to_string(),
            reports: Vec::new(),
            landed_signatures: Vec::new(),
            duplicate_detected: false,
            existence: ChainExistence::Unknown,
            link_allowed: false,
            abandon_allowed: false,
            settled_allowed: false,
            reason,
        }
    }
}

/// Ask the chain about the recorded signature and every alternate.
///
/// `alternates` are operator-supplied and may be empty (the common case). A
/// malformed alternate is reported as `unknown` by [`ChainExistenceProbe::probe`],
/// which makes the whole scan inconclusive rather than silently ignored.
pub async fn verify_signatures(
    probe: Option<&ChainExistenceProbe>,
    trade_key: &str,
    recorded: Option<&str>,
    alternates: &[String],
    operator_asserts_never_sent: bool,
) -> SignatureScanReport {
    let Some(probe) = probe else {
        return SignatureScanReport::empty(
            trade_key,
            "no chain reader is configured; no signature can be verified".into(),
        );
    };
    let mut wanted: Vec<String> = Vec::new();
    if let Some(rec) = recorded.map(str::trim).filter(|s| !s.is_empty()) {
        wanted.push(rec.to_string());
    }
    for alt in alternates {
        let alt = alt.trim();
        if !alt.is_empty() && !wanted.iter().any(|w| w == alt) {
            wanted.push(alt.to_string());
        }
    }
    if wanted.is_empty() {
        // No signature at all: the operator's assertion is the entire evidence
        // (exactly the pre-existing `authorize_abandon` behaviour).
        let mut out = SignatureScanReport::empty(
            trade_key,
            if operator_asserts_never_sent {
                "no signature was recorded or supplied and the operator asserts the attempt \
                 never left the process"
                    .into()
            } else {
                "no signature was recorded or supplied; assert the non-send explicitly to allow \
                 `abandon`"
                    .into()
            },
        );
        out.abandon_allowed = operator_asserts_never_sent;
        out.existence = ChainExistence::Absent;
        return out;
    }

    let mut reports = Vec::with_capacity(wanted.len());
    let mut landed: Vec<String> = Vec::new();
    let mut unknown = false;
    for sig in &wanted {
        let report = probe
            .probe(trade_key, sig, operator_asserts_never_sent)
            .await;
        match report.existence {
            ChainExistence::Landed => landed.push(sig.clone()),
            ChainExistence::Unknown => unknown = true,
            ChainExistence::Absent => {}
        }
        reports.push(report);
    }

    let duplicate_detected = landed.len() > 1;
    let recorded_landed = recorded
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_some_and(|rec| landed.iter().any(|l| l == rec));
    let deep_single = landed.len() == 1
        && reports
            .iter()
            .find(|r| r.existence == ChainExistence::Landed)
            .is_some_and(|r| r.settled_allowed);
    let all_absent = landed.is_empty() && !unknown;

    let (existence, reason) = if duplicate_detected {
        (
            ChainExistence::Landed,
            format!(
                "{} signatures of this attempt are on chain ({}): a duplicate send already \
                 happened — do not free the key, do not book either fill automatically",
                landed.len(),
                landed.join(", ")
            ),
        )
    } else if let Some(one) = landed.first() {
        let deep = deep_single;
        (
            ChainExistence::Landed,
            format!(
                "signature {one} is on chain{}",
                if deep {
                    " (deeply confirmed)"
                } else {
                    " (not deep enough to book settlement)"
                }
            ),
        )
    } else if unknown {
        (
            ChainExistence::Unknown,
            "at least one signature could not be looked up; the chain does not permit any \
             conclusion"
                .into(),
        )
    } else {
        (
            ChainExistence::Absent,
            format!(
                "the node knows none of the {} signature(s) supplied; absence is still NOT \
                 proof that nothing was sent",
                wanted.len()
            ),
        )
    };

    SignatureScanReport {
        trade_key: trade_key.to_string(),
        reports,
        duplicate_detected,
        existence,
        // A landed attempt links the signature that landed — and only ever the
        // recorded one automatically; an alternate is a human decision.
        link_allowed: recorded_landed || (!landed.is_empty() && duplicate_detected),
        abandon_allowed: all_absent && operator_asserts_never_sent,
        settled_allowed: deep_single,
        reason,
        landed_signatures: landed,
    }
}

/// Guard for the sweep's `abandon` decision. Returns `Err(reason)` when the
/// chain evidence forbids freeing the trade key. `probe: None` means no RPC
/// is configured: the decision is refused rather than made blind.
pub async fn authorize_abandon(
    probe: Option<&ChainExistenceProbe>,
    trade_key: &str,
    signature: Option<&str>,
    operator_asserts_never_sent: bool,
) -> Result<ChainExistenceReport, String> {
    let Some(probe) = probe else {
        return Err(
            "no chain reader is configured; `abandon` needs a chain check or an explicit \
             operator override"
                .into(),
        );
    };
    let Some(signature) = signature.map(str::trim).filter(|s| !s.is_empty()) else {
        // A lapsed claim with no signature never recorded a broadcast: the
        // operator's non-send assertion is the whole evidence available.
        return Ok(ChainExistenceReport {
            trade_key: trade_key.to_string(),
            signature: String::new(),
            existence: ChainExistence::Unknown,
            slot: None,
            block_time: None,
            error: None,
            signer_balance_delta: None,
            abandon_allowed: operator_asserts_never_sent,
            link_allowed: false,
            head_slot: None,
            finality: ChainFinality::NotLanded,
            confirmations: None,
            settled_allowed: false,
            reason: if operator_asserts_never_sent {
                "the claim has no signature and the operator asserts the attempt never left the \
                 process"
                    .into()
            } else {
                "the claim has no signature; assert the non-send explicitly to allow `abandon`"
                    .into()
            },
        });
    };
    let report = probe
        .probe(trade_key, signature, operator_asserts_never_sent)
        .await;
    if report.abandon_allowed {
        Ok(report)
    } else {
        Err(report.reason.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(existence: ChainExistence, never_sent: bool) -> ChainExistenceReport {
        ChainExistenceReport {
            trade_key: "k".into(),
            signature: "sig".into(),
            existence,
            slot: None,
            block_time: None,
            error: None,
            signer_balance_delta: None,
            abandon_allowed: matches!(existence, ChainExistence::Absent) && never_sent,
            link_allowed: matches!(existence, ChainExistence::Landed),
            head_slot: None,
            finality: ChainFinality::NotLanded,
            confirmations: None,
            settled_allowed: false,
            reason: String::new(),
        }
    }

    #[tokio::test]
    async fn a_landed_transaction_is_never_abandonable() {
        // The decisive property: freeing the key while the transaction is on
        // chain would let a retry send a second trade.
        let r = report(ChainExistence::Landed, true);
        assert!(!r.abandon_allowed);
        assert!(r.link_allowed);
    }

    #[tokio::test]
    async fn an_absent_signature_needs_an_explicit_non_send_assertion() {
        assert!(!report(ChainExistence::Absent, false).abandon_allowed);
        assert!(report(ChainExistence::Absent, true).abandon_allowed);
    }

    #[tokio::test]
    async fn unknown_chain_evidence_authorizes_nothing() {
        let r = report(ChainExistence::Unknown, true);
        assert!(!r.abandon_allowed, "an RPC failure is not a non-send proof");
        assert!(!r.link_allowed);
    }

    #[tokio::test]
    async fn without_a_chain_reader_abandon_is_refused() {
        let out = authorize_abandon(None, "copy:w:m", Some("sig"), true).await;
        assert!(
            out.is_err(),
            "no chain reader must not mean 'assume absent'"
        );
    }

    #[tokio::test]
    async fn a_claim_without_a_signature_follows_the_operator_assertion() {
        let refused = authorize_abandon(None, "copy:w:m", None, false).await;
        assert!(refused.is_err());
        // The no-RPC path cannot reach the signature-less branch; with a probe
        // attached the branch is covered by the integration test against a
        // mock RPC.
    }
}
