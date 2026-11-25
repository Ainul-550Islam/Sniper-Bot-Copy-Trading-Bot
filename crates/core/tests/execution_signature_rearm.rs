//! Regression: a late verdict for an old attempt must not settle its replacement.
use bot_core::execution::{ExecutionIntent, ExecutionLedger, ExecutionState, FailureClass};

#[tokio::test]
async fn stale_signature_does_not_reconcile_a_new_attempt() {
    let ledger = ExecutionLedger::new(64);
    ledger
        .begin(ExecutionIntent {
            intent_id: "same-intent".into(),
            module: "test".into(),
            label: "test".into(),
            wallet: "wallet".into(),
            symbol: "symbol".into(),
        })
        .await
        .unwrap();
    ledger
        .transition("same-intent", ExecutionState::Validated, None)
        .await
        .unwrap();
    ledger
        .attach_submission("same-intent", "old-signature", None, None, 0)
        .await
        .unwrap();
    ledger
        .transition("same-intent", ExecutionState::Submitted, None)
        .await
        .unwrap();
    ledger
        .fail("same-intent", FailureClass::BlockhashExpired, "expired")
        .await
        .unwrap();
    ledger
        .begin(ExecutionIntent {
            intent_id: "same-intent".into(),
            module: "test".into(),
            label: "test".into(),
            wallet: "wallet".into(),
            symbol: "symbol".into(),
        })
        .await
        .unwrap();
    ledger
        .transition("same-intent", ExecutionState::Validated, None)
        .await
        .unwrap();
    ledger
        .attach_submission("same-intent", "new-signature", None, None, 0)
        .await
        .unwrap();
    ledger
        .transition("same-intent", ExecutionState::Submitted, None)
        .await
        .unwrap();
    assert!(
        ledger
            .reconcile_signature("old-signature", false, "late old verdict")
            .await
            .is_none(),
        "old verdict must not affect the replacement attempt"
    );
    let current = ledger.get("same-intent").await.unwrap();
    assert_eq!(current.state, ExecutionState::Submitted);
    assert_eq!(current.signature.as_deref(), Some("new-signature"));
    assert!(ledger
        .reconcile_signature("new-signature", true, "landed")
        .await
        .is_some());
}
