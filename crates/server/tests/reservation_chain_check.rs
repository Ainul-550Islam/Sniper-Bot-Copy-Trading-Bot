//! Chain-side existence check for an ambiguous pre-broadcast claim.
//!
//! The sweep lets an operator free a trade key (`abandon`), which lets a
//! RETRY send. That is only safe when the earlier transaction is not on
//! chain. These tests drive the production probe against an in-process mock
//! JSON-RPC node and pin all three answers:
//!
//! * `landed` — the node returns the transaction: `abandon` is forbidden,
//!   `link` is the only allowed decision;
//! * `absent` — the node answers `null`: NOT proof of a non-send, so
//!   `abandon` stays forbidden until the operator explicitly asserts the
//!   attempt never left the process;
//! * `unknown` — the node errors: never treated as absent.
//!
//! Nothing here is live-chain evidence; the RPC is a local mock.

use std::sync::Arc;

use axum::{routing::post, Router};
use bot_core::config::NetworkConfig;
use serde_json::{json, Value};
use sniper_suite::reservation_chain::{
    verify_signatures, ChainExistence, ChainExistenceProbe, ChainFinality,
};

/// A real serialised transaction, base64-encoded, so the client decodes the
/// body exactly as it would a provider answer.
fn encoded_transaction() -> String {
    use base64::Engine;
    use solana_sdk::signature::Signer;
    let payer = solana_sdk::signature::Keypair::new();
    let ix = solana_sdk::instruction::Instruction {
        program_id: solana_sdk::pubkey::Pubkey::new_unique(),
        accounts: vec![],
        data: vec![],
    };
    let tx = solana_sdk::transaction::Transaction::new_signed_with_payer(
        &[ix],
        Some(&payer.pubkey()),
        &[&payer],
        solana_sdk::hash::Hash::new_unique(),
    );
    base64::engine::general_purpose::STANDARD.encode(bincode::serialize(&tx).unwrap())
}

/// One mock JSON-RPC node whose `getTransaction` answer is scripted.
async fn mock_rpc(answer: Result<Value, ()>) -> NetworkConfig {
    mock_rpc_with_head(answer, None).await
}

/// Same node, but also answers `getSlot` with `head` (`None` = the node has no
/// slot to report, which must be reported as an unknown depth).
async fn mock_rpc_with_head(answer: Result<Value, ()>, head: Option<u64>) -> NetworkConfig {
    let state = Arc::new(tokio::sync::Mutex::new(Some(answer)));
    let app = Router::new().route(
        "/",
        post(move |body: axum::Json<Value>| {
            let state = state.clone();
            async move {
                let method = body.0.get("method").and_then(Value::as_str).unwrap_or("");
                let id = body.0.get("id").cloned().unwrap_or(json!(1));
                if method == "getSlot" {
                    return match head {
                        Some(h) => axum::Json(json!({ "jsonrpc": "2.0", "id": id, "result": h })),
                        None => axum::Json(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": { "code": -32000, "message": "no slot" }
                        })),
                    };
                }
                if method != "getTransaction" {
                    // Any other call is answered empty so the probe only
                    // depends on what it says it needs.
                    return axum::Json(json!({ "jsonrpc": "2.0", "id": id, "result": null }));
                }
                let mut guard = state.lock().await;
                match guard.take() {
                    Some(Ok(result)) => {
                        axum::Json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
                    }
                    Some(Err(())) => axum::Json(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32000, "message": "node unavailable" }
                    })),
                    None => axum::Json(json!({ "jsonrpc": "2.0", "id": id, "result": null })),
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::task::yield_now().await;

    NetworkConfig {
        rpc_url: format!("http://{addr}"),
        commitment: "confirmed".into(),
        ..Default::default()
    }
}

const SIG: &str =
    "5VERv8NMvzbJMEkV8xnrLkEaWRtSz9CosKDYjCJjBRnbJLgp8uirBgmQpjKhoR4tjF3ZpRzrFmBV6UjKdiSZkQUW";

/// A minimal `getTransaction` body the client decodes: base64 message,
/// `meta` with balances + `err: null`, slot and block time.
fn landed_transaction(slot: u64) -> Value {
    json!({
        "slot": slot,
        "blockTime": 1_760_000_000i64,
        // The result is FLATTENED: the transaction body and its meta are
        // siblings of `slot`, exactly as a provider answers.
        "transaction": [encoded_transaction(), "base64"],
        "meta": {
            "err": null,
            "fee": 5000,
            "preBalances": [10_000_000u64],
            "postBalances": [9_000_000u64],
            "preTokenBalances": [],
            "postTokenBalances": [],
            "innerInstructions": [],
            "logMessages": [],
            "rewards": [],
            "status": { "Ok": null },
            "loadedAddresses": { "writable": [], "readonly": [] }
        }
    })
}

#[tokio::test]
async fn a_landed_transaction_forbids_abandoning_the_trade_key() {
    let cfg = mock_rpc(Ok(landed_transaction(424_242))).await;
    let probe = ChainExistenceProbe::from_config(&cfg).expect("probe");
    let report = probe.probe("copy:w:m", SIG, true).await;

    assert_eq!(report.existence, ChainExistence::Landed, "{report:?}");
    assert_eq!(report.slot, Some(424_242));
    assert_eq!(report.block_time, Some(1_760_000_000));
    assert_eq!(report.error, None);
    assert_eq!(report.signer_balance_delta, Some(-1_000_000));
    assert!(
        !report.abandon_allowed,
        "a transaction on chain must never free the key for a retry"
    );
    assert!(report.link_allowed);
    assert!(report.reason.contains("on chain"), "{}", report.reason);
}

#[tokio::test]
async fn an_absent_signature_needs_the_operator_to_assert_the_non_send() {
    let cfg = mock_rpc(Ok(Value::Null)).await;
    let probe = ChainExistenceProbe::from_config(&cfg).expect("probe");

    let cautious = probe.probe("copy:w:m", SIG, false).await;
    assert_eq!(cautious.existence, ChainExistence::Absent);
    assert!(
        !cautious.abandon_allowed,
        "an unknown signature is not proof that nothing was sent"
    );
    assert!(!cautious.link_allowed);
    assert!(cautious.reason.contains("NOT proof"), "{}", cautious.reason);

    let asserted = probe.probe("copy:w:m", SIG, true).await;
    assert_eq!(asserted.existence, ChainExistence::Absent);
    assert!(
        asserted.abandon_allowed,
        "an explicit non-send assertion may free the key"
    );
    assert!(!asserted.link_allowed);
}

#[tokio::test]
async fn a_failed_chain_lookup_is_unknown_and_authorizes_nothing() {
    let cfg = mock_rpc(Err(())).await;
    let probe = ChainExistenceProbe::from_config(&cfg).expect("probe");
    let report = probe.probe("copy:w:m", SIG, true).await;
    assert_eq!(report.existence, ChainExistence::Unknown, "{report:?}");
    assert!(!report.abandon_allowed, "an RPC failure is not a non-send");
    assert!(!report.link_allowed);
    assert!(report.reason.contains("lookup failed"), "{}", report.reason);
}

#[tokio::test]
async fn a_malformed_signature_is_refused_without_calling_the_node() {
    let cfg = mock_rpc(Ok(landed_transaction(1))).await;
    let probe = ChainExistenceProbe::from_config(&cfg).expect("probe");
    let report = probe.probe("copy:w:m", "not-a-signature", true).await;
    assert_eq!(report.existence, ChainExistence::Unknown);
    assert!(!report.abandon_allowed);
    assert!(report.reason.contains("not a valid transaction signature"));
}

#[tokio::test]
async fn a_shallow_landing_is_linkable_but_never_settleable() {
    // 20 slots behind the head: on chain, so `abandon` stays forbidden, but it
    // can still be rolled back, so no economics may be booked from it.
    let cfg = mock_rpc_with_head(Ok(landed_transaction(1000)), Some(1020)).await;
    let probe = ChainExistenceProbe::from_config(&cfg).expect("probe");
    let report = probe.probe("copy:w:m", SIG, false).await;

    assert_eq!(report.existence, ChainExistence::Landed);
    assert!(report.link_allowed);
    assert!(
        !report.abandon_allowed,
        "a landed transaction is never abandonable"
    );
    assert_eq!(report.head_slot, Some(1020));
    assert_eq!(report.confirmations, Some(20));
    assert_eq!(report.finality, ChainFinality::Unfinalized);
    assert!(
        !report.settled_allowed,
        "a reversible landing must not be booked as settled"
    );
    assert!(
        report.reason.contains("still reversible"),
        "{}",
        report.reason
    );
}

#[tokio::test]
async fn a_deep_landing_is_settleable_but_still_not_abandonable() {
    let cfg = mock_rpc_with_head(Ok(landed_transaction(1000)), Some(2000)).await;
    let probe = ChainExistenceProbe::from_config(&cfg).expect("probe");
    let report = probe.probe("copy:w:m", SIG, false).await;

    assert_eq!(report.finality, ChainFinality::DeeplyConfirmed);
    assert_eq!(report.confirmations, Some(1000));
    assert!(report.settled_allowed);
    assert!(report.link_allowed);
    assert!(
        !report.abandon_allowed,
        "depth never authorises freeing the trade key"
    );
}

#[tokio::test]
async fn an_unknown_head_leaves_the_landing_unsettled() {
    // The node cannot report its slot: the landing is still proof for `link`,
    // but the depth is unknown, so nothing may be booked.
    let cfg = mock_rpc_with_head(Ok(landed_transaction(1000)), None).await;
    let probe = ChainExistenceProbe::from_config(&cfg).expect("probe");
    let report = probe.probe("copy:w:m", SIG, false).await;

    assert_eq!(report.existence, ChainExistence::Landed);
    assert_eq!(report.head_slot, None);
    assert_eq!(report.confirmations, None);
    assert_eq!(report.finality, ChainFinality::Unfinalized);
    assert!(!report.settled_allowed);
    assert!(report.reason.contains("depth unknown"), "{}", report.reason);
}

#[tokio::test]
async fn a_landing_ahead_of_the_reported_head_is_not_settleable() {
    // A lagging node whose head is BELOW the transaction slot: the depth is
    // not established, so this must not be treated as deep.
    let cfg = mock_rpc_with_head(Ok(landed_transaction(5000)), Some(4000)).await;
    let probe = ChainExistenceProbe::from_config(&cfg).expect("probe");
    let report = probe.probe("copy:w:m", SIG, false).await;

    assert_eq!(report.existence, ChainExistence::Landed);
    assert_eq!(report.confirmations, None);
    assert_eq!(report.finality, ChainFinality::Unfinalized);
    assert!(!report.settled_allowed);
}

#[tokio::test]
async fn absence_and_errors_are_never_settleable() {
    for (answer, head) in [
        (Ok(Value::Null), Some(9_999_999u64)),
        (Err(()), Some(9_999_999u64)),
    ] {
        let cfg = mock_rpc_with_head(answer, head).await;
        let probe = ChainExistenceProbe::from_config(&cfg).expect("probe");
        let report = probe.probe("copy:w:m", SIG, true).await;
        assert!(!report.link_allowed, "{report:?}");
        assert!(!report.settled_allowed, "{report:?}");
        assert_eq!(report.finality, ChainFinality::NotLanded, "{report:?}");
    }
}

/// A real, distinct, VALID transaction signature for the "alternate" slots
/// (a pubkey string is 32 bytes and would fail `Signature::from_str`, which is
/// exactly how a malformed alternate is reported as `unknown`).
fn other_signature() -> String {
    use solana_sdk::signature::Signer;
    solana_sdk::signature::Keypair::new()
        .sign_message(b"alternate attempt")
        .to_string()
}

/// A node that answers `getTransaction` only for the signatures it was told to
/// know, and always reports a deep head.
async fn mock_known_transactions(known: Vec<String>, head: u64) -> NetworkConfig {
    let known = Arc::new(known);
    let app = Router::new().route(
        "/",
        post(move |body: axum::Json<Value>| {
            let known = known.clone();
            async move {
                let method = body.0.get("method").and_then(Value::as_str).unwrap_or("");
                let id = body.0.get("id").cloned().unwrap_or(json!(1));
                let result = match method {
                    "getSlot" => json!(head),
                    "getTransaction" => {
                        let sig = body
                            .0
                            .get("params")
                            .and_then(|p| p.get(0))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        if known.iter().any(|k| k == &sig) {
                            landed_transaction(1_000)
                        } else {
                            Value::Null
                        }
                    }
                    _ => Value::Null,
                };
                axum::Json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::task::yield_now().await;
    NetworkConfig {
        rpc_url: format!("http://{addr}"),
        commitment: "confirmed".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn one_landed_signature_is_linkable_and_never_abandonable() {
    let alt = other_signature();
    let cfg = mock_known_transactions(vec![SIG.to_string()], 5_000).await;
    let probe = ChainExistenceProbe::from_config(&cfg).expect("probe");
    let scan = verify_signatures(Some(&probe), "copy:w:m", Some(SIG), &[alt], true).await;

    assert_eq!(scan.landed_signatures, vec![SIG.to_string()]);
    assert!(!scan.duplicate_detected);
    assert_eq!(scan.existence, ChainExistence::Landed);
    assert!(scan.link_allowed);
    assert!(
        !scan.abandon_allowed,
        "an explicit non-send assertion never beats chain evidence"
    );
    assert!(scan.settled_allowed, "a deep single landing may be booked");
    assert_eq!(scan.reports.len(), 2, "both signatures were asked about");
}

#[tokio::test]
async fn two_landed_signatures_are_reported_as_a_duplicate_and_nothing_is_settleable() {
    let alt = other_signature();
    let cfg = mock_known_transactions(vec![SIG.to_string(), alt.clone()], 5_000).await;
    let probe = ChainExistenceProbe::from_config(&cfg).expect("probe");
    let scan = verify_signatures(
        Some(&probe),
        "copy:w:m",
        Some(SIG),
        std::slice::from_ref(&alt),
        true,
    )
    .await;

    assert_eq!(scan.landed_signatures.len(), 2, "{scan:?}");
    assert!(
        scan.duplicate_detected,
        "two signatures of one attempt on chain is the duplicate the fence exists to prevent"
    );
    assert!(!scan.abandon_allowed, "nothing may be freed");
    assert!(
        !scan.settled_allowed,
        "a duplicate must never be booked automatically"
    );
    assert!(
        scan.reason.contains("duplicate send already happened"),
        "{}",
        scan.reason
    );
}

#[tokio::test]
async fn an_alternate_that_is_the_one_on_chain_is_not_reported_as_absent() {
    // The recorded signature never landed; the re-signed attempt did. Asking
    // only about the recorded one would have said `absent`.
    let alt = other_signature();
    let cfg = mock_known_transactions(vec![alt.clone()], 5_000).await;
    let probe = ChainExistenceProbe::from_config(&cfg).expect("probe");
    let scan = verify_signatures(
        Some(&probe),
        "copy:w:m",
        Some(SIG),
        std::slice::from_ref(&alt),
        false,
    )
    .await;

    assert_eq!(scan.existence, ChainExistence::Landed, "{scan:?}");
    assert_eq!(scan.landed_signatures, vec![alt.clone()]);
    assert!(!scan.duplicate_detected);
    assert!(
        !scan.link_allowed,
        "linking an alternate is a human decision, not an automatic one"
    );
    assert!(!scan.abandon_allowed, "a landing always forbids abandon");
    assert!(scan.settled_allowed, "a single deep landing is settleable");
}

#[tokio::test]
async fn an_unlookupable_alternate_makes_the_scan_inconclusive() {
    // The recorded signature is absent and the alternate is malformed: the
    // scan must not fall back to "nothing landed".
    let cfg = mock_known_transactions(vec![], 5_000).await;
    let probe = ChainExistenceProbe::from_config(&cfg).expect("probe");
    let scan = verify_signatures(
        Some(&probe),
        "copy:w:m",
        Some(SIG),
        &["not-a-signature".to_string()],
        true,
    )
    .await;

    assert_eq!(scan.existence, ChainExistence::Unknown, "{scan:?}");
    assert!(
        !scan.abandon_allowed,
        "an inconclusive scan never frees a key"
    );
    assert!(!scan.link_allowed);
    assert!(!scan.settled_allowed);
}

#[tokio::test]
async fn a_scan_with_no_signature_follows_the_operator_assertion() {
    let cfg = mock_known_transactions(vec![], 5_000).await;
    let probe = ChainExistenceProbe::from_config(&cfg).expect("probe");

    let cautious = verify_signatures(Some(&probe), "copy:w:m", None, &[], false).await;
    assert!(!cautious.abandon_allowed);
    assert_eq!(cautious.existence, ChainExistence::Absent);

    let asserted = verify_signatures(Some(&probe), "copy:w:m", None, &[], true).await;
    assert!(asserted.abandon_allowed);
    assert!(!asserted.link_allowed && !asserted.settled_allowed);
}

#[tokio::test]
async fn a_scan_without_a_chain_reader_concludes_nothing() {
    let scan = verify_signatures(None, "copy:w:m", Some(SIG), &[], true).await;
    assert_eq!(scan.existence, ChainExistence::Unknown);
    assert!(!scan.abandon_allowed && !scan.link_allowed && !scan.settled_allowed);
    assert!(scan.reason.contains("no chain reader"), "{}", scan.reason);
}
