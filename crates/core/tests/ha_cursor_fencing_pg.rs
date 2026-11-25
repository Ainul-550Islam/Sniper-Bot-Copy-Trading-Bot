//! Real-Postgres HA cursor fencing: the SQL admission rules must agree with
//! the in-memory ones. No live feed, no venue, no cluster — just the durable
//! row and the writers that try to move it.
//!
//! Covers the two OPEN gaps
//!   * "HA cursor fencing" — a blind upsert becomes a fenced, monotonic write;
//!   * "HA cursor advance before processing and unfenced writes" — an advance
//!     is written `pending` and only acknowledged after the side effect, so a
//!     crash replays instead of skipping.
mod trading_isolation_common;

use std::sync::Arc;

use bot_core::db::ha::HaRepo;
use bot_core::ha::{CursorAdvance, CursorWrite, FeedCursor, FeedId};
use chrono::Utc;
use trading_isolation_common::{run_id, setup};

fn cursor(run: &str, fence: &str, pending: bool, position: u64) -> FeedCursor {
    let mut c = FeedCursor::new(FeedId::CopyLogs, run, "w");
    c.fence = fence.to_string();
    c.offer(position, None, "w", Utc::now());
    c.pending = pending;
    c
}

#[tokio::test]
async fn cursor_rows_are_fenced_monotonic_and_replayable() {
    let Some(db) = setup().await else {
        eprintln!("NOT_RUN: ha_cursor_fencing_pg — POSTGRES_URL missing");
        return;
    };
    let run = run_id();
    let repo = HaRepo::new(Arc::clone(&db));

    // 1. First write wins an absent row and lands as PENDING (the caller has
    //    not done the work yet).
    let first = cursor(&run, "w-a@1", true, 10);
    assert_eq!(
        repo.save_cursor(&first).await.unwrap(),
        CursorWrite::Accepted
    );
    let row = repo.load_cursor(&first.key()).await.unwrap().expect("row");
    assert!(row.pending, "an unconfirmed advance is pending");
    assert_eq!(row.fence, "w-a@1");
    assert_eq!(row.position, Some(10));

    // 2. The pending advance shows up in the replay set.
    let pending = repo.pending_cursors(1000).await.unwrap();
    assert!(pending.iter().any(|c| c.key() == first.key()));

    // 3. A DIFFERENT holder at the same generation (split brain) is fenced.
    let split = cursor(&run, "w-b@1", true, 11);
    assert!(matches!(
        repo.save_cursor(&split).await.unwrap(),
        CursorWrite::Fenced { .. }
    ));
    // 4. An OLDER generation is fenced whatever its position.
    let old = cursor(&run, "w-a@0", true, 99);
    assert!(matches!(
        repo.save_cursor(&old).await.unwrap(),
        CursorWrite::Fenced { .. }
    ));
    // 5. An unfenced writer never displaces a fenced row.
    let mut unfenced = cursor(&run, "", true, 99);
    unfenced.fence = String::new();
    assert!(matches!(
        repo.save_cursor(&unfenced).await.unwrap(),
        CursorWrite::Fenced { .. }
    ));
    // The durable row is untouched by every refusal.
    let row = repo.load_cursor(&first.key()).await.unwrap().unwrap();
    assert_eq!((row.position, row.fence.as_str()), (Some(10), "w-a@1"));

    // 6. An unacknowledged advance is never overwritten backwards.
    let behind = cursor(&run, "w-a@1", true, 4);
    assert!(matches!(
        repo.save_cursor(&behind).await.unwrap(),
        CursorWrite::Stale { .. }
    ));

    // 7. A takeover (generation 2) may REPLAY the same pending position...
    let mut replay = cursor(&run, "w-a@2", false, 10);
    replay.pending = false;
    assert_eq!(
        repo.save_cursor(&replay).await.unwrap(),
        CursorWrite::Accepted,
        "a newer generation may supersede the pending advance"
    );
    // ...and cannot be acknowledged by the stale generation afterwards.
    let mut again = cursor(&run, "w-a@2", true, 10);
    again.pending = true;
    assert_eq!(
        repo.save_cursor(&again).await.unwrap(),
        CursorWrite::Accepted
    );
    assert!(
        !repo
            .acknowledge_cursor(&first.key(), Some(10), None, "w-a@1")
            .await
            .unwrap(),
        "a stale generation never acknowledges a newer owner's advance"
    );
    // 8. The owner acknowledges; the row is no longer pending and adopts the
    //    acknowledging generation as its fence.
    assert!(repo
        .acknowledge_cursor(&first.key(), Some(10), None, "w-a@2")
        .await
        .unwrap());
    let row = repo.load_cursor(&first.key()).await.unwrap().unwrap();
    assert!(!row.pending);
    assert_eq!(row.fence, "w-a@2");
    assert!(!repo
        .pending_cursors(1000)
        .await
        .unwrap()
        .iter()
        .any(|c| c.key() == first.key()));
    // 9. Acknowledging twice is not a new fact.
    assert!(!repo
        .acknowledge_cursor(&first.key(), Some(10), None, "w-a@2")
        .await
        .unwrap());

    // 10. Acknowledged rows advance forward, and the counters survive.
    let mut next = cursor(&run, "w-a@2", true, 11);
    next.processed_count = row.processed_count + 1;
    assert_eq!(
        repo.save_cursor(&next).await.unwrap(),
        CursorWrite::Accepted
    );
    let row = repo.load_cursor(&first.key()).await.unwrap().unwrap();
    assert_eq!(row.position, Some(11));
    assert_eq!(row.processed_count, next.processed_count);

    // 11. A rewind is admitted over an acknowledged row (deliberate replay).
    let mut rewind = row.clone();
    rewind.rewind_to(Some(2), Utc::now());
    rewind.fence = "w-a@2".into();
    assert_eq!(
        repo.save_cursor(&rewind).await.unwrap(),
        CursorWrite::Accepted
    );
    assert_eq!(
        repo.load_cursor(&first.key())
            .await
            .unwrap()
            .unwrap()
            .position,
        Some(2)
    );

    // 12. The CursorAdvance vocabulary still says a refusal is not processed.
    assert!(!CursorAdvance::Refused {
        reason: "fenced".into(),
        detail: String::new()
    }
    .should_process());
}
