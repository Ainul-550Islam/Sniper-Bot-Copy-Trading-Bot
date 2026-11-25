//! Durable feed cursors, gap detection and replay bookkeeping
//! (TASK 6 §4).
//!
//! Every event feed the suite consumes advances a cursor that is persisted,
//! so a restart resumes from the last durable position instead of from
//! "now" (which silently drops everything that happened while the process
//! was down) or from the beginning (which replays work).
//!
//! Two cursor shapes cover every feed the suite has:
//!
//! | shape | feeds | ordering key |
//! |---|---|---|
//! | sequence | Polymarket user channel, websocket streams with a monotonic id | `u64` sequence |
//! | opaque | Solana signature polling (sniper / copy) | last processed signature + observed time |
//!
//! Both record `processed_count`, the last event's time and the last
//! durable write, so cursor lag is observable.
//!
//! **Gaps are never silently skipped** (§4). A sequence feed that jumps
//! from 41 to 45 records a [`FeedGap`] describing the missing range; the
//! consumer keeps going (dropping the stream would be worse) but the gap is
//! journaled, audited, metered and visible to operators until it is
//! backfilled or explicitly accepted. Replay of an already-processed
//! position is suppressed by the cursor itself and counted.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// The feeds that carry durable cursors. Closed vocabulary — bounded metric
/// label and a reviewable list of everything that can lose events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedId {
    /// Sniper launch detection (pump.fun logs / PumpPortal).
    SniperLaunches,
    /// Copy-trading leader signature polling.
    CopyLogs,
    /// Copy-trading Geyser `transactionSubscribe` stream.
    CopyGeyser,
    /// Polymarket market (book) websocket.
    PolymarketMarket,
    /// Polymarket authenticated user channel (orders + trades).
    PolymarketUser,
}

impl FeedId {
    /// Every feed, stable order.
    pub const ALL: [FeedId; 5] = [
        FeedId::SniperLaunches,
        FeedId::CopyLogs,
        FeedId::CopyGeyser,
        FeedId::PolymarketMarket,
        FeedId::PolymarketUser,
    ];

    /// Stable label (durable key, metric label, lease role name).
    pub fn as_str(&self) -> &'static str {
        match self {
            FeedId::SniperLaunches => "sniper_launches",
            FeedId::CopyLogs => "copy_logs",
            FeedId::CopyGeyser => "copy_geyser",
            FeedId::PolymarketMarket => "polymarket_market",
            FeedId::PolymarketUser => "polymarket_user",
        }
    }

    /// Inverse of [`FeedId::as_str`].
    pub fn parse(s: &str) -> Option<FeedId> {
        FeedId::ALL.iter().copied().find(|f| f.as_str() == s)
    }

    /// Does this feed carry a monotonic sequence?
    pub fn is_sequenced(&self) -> bool {
        matches!(self, FeedId::PolymarketUser | FeedId::CopyGeyser)
    }
}

impl std::fmt::Display for FeedId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One durable cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeedCursor {
    /// Which feed.
    pub feed: FeedId,
    /// Optional scope inside the feed (leader wallet, market id, …).
    /// Empty = the whole feed.
    pub scope: String,
    /// Last processed sequence for sequenced feeds.
    pub position: Option<u64>,
    /// Last processed opaque token (signature, event id) for unsequenced
    /// feeds.
    pub token: Option<String>,
    /// Event time of the last processed item (lag input).
    pub last_event_at: Option<DateTime<Utc>>,
    /// When the cursor was last persisted.
    pub updated_at: DateTime<Utc>,
    /// Total items this cursor advanced over.
    pub processed_count: i64,
    /// Items refused because they were at or behind the cursor.
    pub duplicate_count: i64,
    /// Gaps observed on this cursor.
    pub gap_count: i64,
    /// Worker that last advanced it.
    pub worker_id: String,
    /// The advance above was recorded but the caller has not yet confirmed
    /// that the work for it completed. A crash with `pending = true` means
    /// "replay from the last acknowledged position", never "resume after
    /// this one": the position may be ahead of the side effects.
    pub pending: bool,
    /// Fencing token of the writer: `<holder>@<generation>`, empty when the
    /// process runs without cluster leases. A cursor write from an older
    /// generation than the row already carries is refused
    /// (see [`CursorWrite`]).
    pub fence: String,
}

impl FeedCursor {
    /// A fresh, empty cursor.
    pub fn new(feed: FeedId, scope: impl Into<String>, worker_id: impl Into<String>) -> Self {
        FeedCursor {
            feed,
            scope: scope.into(),
            position: None,
            token: None,
            last_event_at: None,
            updated_at: Utc::now(),
            processed_count: 0,
            duplicate_count: 0,
            gap_count: 0,
            worker_id: worker_id.into(),
            pending: false,
            fence: String::new(),
        }
    }

    /// Durable key: `feed[:scope]`.
    pub fn key(&self) -> String {
        if self.scope.is_empty() {
            self.feed.as_str().to_string()
        } else {
            format!("{}:{}", self.feed.as_str(), self.scope)
        }
    }

    /// Seconds between the last event and `now` (cursor lag). `None` when
    /// nothing was processed yet.
    pub fn lag_secs(&self, now: DateTime<Utc>) -> Option<i64> {
        self.last_event_at
            .map(|t| now.signed_duration_since(t).num_seconds().max(0))
    }

    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!(
            "feed={} scope={} position={} token={} processed={} duplicates={} gaps={} worker={}",
            self.feed,
            if self.scope.is_empty() {
                "-"
            } else {
                &self.scope
            },
            self.position
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".into()),
            self.token.as_deref().unwrap_or("-"),
            self.processed_count,
            self.duplicate_count,
            self.gap_count,
            self.worker_id
        )
    }
}

/// A detected discontinuity in a sequenced feed. Never silently skipped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeedGap {
    /// Feed the gap is on.
    pub feed: FeedId,
    /// Scope inside the feed.
    pub scope: String,
    /// First missing sequence (inclusive).
    pub from_position: u64,
    /// Last missing sequence (inclusive).
    pub to_position: u64,
    /// When it was detected.
    pub detected_at: DateTime<Utc>,
    /// Worker that detected it.
    pub worker_id: String,
    /// `detected` until a backfill or an explicit operator decision
    /// resolves it.
    pub status: GapStatus,
}

impl FeedGap {
    /// How many events are missing.
    pub fn len(&self) -> u64 {
        self.to_position.saturating_sub(self.from_position) + 1
    }

    /// Always false — a gap record exists only when something is missing.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// Single-line audit text.
    pub fn summary(&self) -> String {
        format!(
            "feed={} scope={} missing={}..={} count={} status={} worker={}",
            self.feed,
            if self.scope.is_empty() {
                "-"
            } else {
                &self.scope
            },
            self.from_position,
            self.to_position,
            self.len(),
            self.status.as_str(),
            self.worker_id
        )
    }
}

/// Lifecycle of a detected gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapStatus {
    /// Observed, not yet explained.
    Detected,
    /// A backfill re-delivered the range.
    Backfilled,
    /// An operator accepted the loss explicitly (audited).
    Accepted,
}

impl GapStatus {
    /// Stable label.
    pub fn as_str(&self) -> &'static str {
        match self {
            GapStatus::Detected => "detected",
            GapStatus::Backfilled => "backfilled",
            GapStatus::Accepted => "accepted",
        }
    }

    /// Inverse of [`GapStatus::as_str`].
    pub fn parse(s: &str) -> Option<GapStatus> {
        match s {
            "detected" => Some(GapStatus::Detected),
            "backfilled" => Some(GapStatus::Backfilled),
            "accepted" => Some(GapStatus::Accepted),
            _ => None,
        }
    }
}

/// What [`FeedCursor::offer`] decided about one incoming item.
#[derive(Debug, Clone, PartialEq)]
pub enum CursorAdvance {
    /// In order; the cursor advanced. Process the item.
    Advanced,
    /// Already processed (at or behind the cursor). Drop it.
    Duplicate,
    /// Ahead of the expected position: the cursor advanced AND a gap is
    /// reported. Process the item — but the gap must not be forgotten.
    Gap(FeedGap),
    /// The durable advance was refused (a newer lease generation owns the
    /// row, or an unacknowledged advance would have been overwritten). The
    /// item must NOT be processed here: nothing was recorded, so dropping it
    /// now would lose it silently. It is re-delivered after replay.
    Refused {
        /// `fenced` | `stale` | `error`.
        reason: String,
        /// Audit detail.
        detail: String,
    },
}

impl CursorAdvance {
    /// Should the caller process the item?
    pub fn should_process(&self) -> bool {
        !matches!(
            self,
            CursorAdvance::Duplicate | CursorAdvance::Refused { .. }
        )
    }

    /// Does this advance leave a side effect to confirm? An advanced or
    /// gap-reported item has not been processed yet when `offer` returns, so
    /// the durable cursor is written as `pending` until
    /// `HaRuntime::acknowledge` says the work completed.
    pub fn needs_ack(&self) -> bool {
        self.should_process()
    }

    /// Stable label for metrics.
    pub fn as_str(&self) -> &'static str {
        match self {
            CursorAdvance::Advanced => "advanced",
            CursorAdvance::Duplicate => "duplicate",
            CursorAdvance::Gap(_) => "gap",
            CursorAdvance::Refused { .. } => "refused",
        }
    }
}

/// What a durable cursor write was allowed to do — the answer to
/// [`FeedCursor::admit_write`]. Every refusal is deterministic and named;
/// nothing here is "last write wins".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorWrite {
    /// The write is newer than the durable row (or the row is absent).
    Accepted,
    /// The presenting writer is stale: the durable row belongs to a newer
    /// lease generation. The write is refused and the durable row is
    /// untouched.
    Fenced {
        /// Fence the store already carries (`<holder>@<generation>`).
        actual: String,
        /// Fence the caller presented.
        presented: String,
    },
    /// The write would move the durable position/token BACKWARDS while an
    /// earlier advance for the same key is still unacknowledged. Refused:
    /// the unacknowledged advance is the one that must be replayed or
    /// acknowledged, not silently overwritten.
    Stale {
        /// Durable position/token description.
        actual: String,
        /// Presented position/token description.
        presented: String,
    },
}

impl CursorWrite {
    /// Did the store accept it?
    pub fn accepted(&self) -> bool {
        matches!(self, CursorWrite::Accepted)
    }

    /// Stable label (metrics / audit).
    pub fn as_str(&self) -> &'static str {
        match self {
            CursorWrite::Accepted => "accepted",
            CursorWrite::Fenced { .. } => "fenced",
            CursorWrite::Stale { .. } => "stale",
        }
    }
}

impl std::fmt::Display for CursorWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CursorWrite::Accepted => f.write_str("accepted"),
            CursorWrite::Fenced { actual, presented } => write!(
                f,
                "fenced: cursor belongs to {actual}, this writer presented {presented}"
            ),
            CursorWrite::Stale { actual, presented } => write!(
                f,
                "stale: an unacknowledged advance at {actual} would be overwritten by {presented}"
            ),
        }
    }
}

/// Generation encoded in a fence (`<holder>@<generation>`). Unparseable or
/// empty fences are generation 0 — a writer that never presented a token can
/// only lose against one that did.
pub fn fence_generation(fence: &str) -> i64 {
    fence_parts(fence).1
}

/// Split a fence string into `(holder, generation)`. Unparseable fences sort
/// as generation 0 (a writer that never presented a token can only lose).
fn fence_parts(fence: &str) -> (&str, i64) {
    match fence.rsplit_once('@') {
        Some((holder, gen)) => (holder, gen.parse::<i64>().unwrap_or(0)),
        None => (fence, 0),
    }
}

impl FeedCursor {
    /// Should this cursor write be admitted over `durable`? Pure, so the
    /// database and the in-memory store cannot disagree.
    ///
    /// Rules, in order:
    ///
    /// 1. No durable row → accepted.
    /// 2. Same fence → accepted (the holder is advancing its own row); a
    ///    lower generation of the SAME holder is stale, a higher one wins.
    /// 3. Different holder → the higher generation wins; equal generations
    ///    are a genuine split brain and the write is refused as `Fenced`
    ///    (the durable row keeps ownership) rather than letting two workers
    ///    interleave positions.
    /// 4. An unacknowledged advance (`durable.pending`) may not be replaced
    ///    by an equal-or-older position/token: that is `Stale`.
    pub fn admit_write(&self, durable: Option<&FeedCursor>) -> CursorWrite {
        let Some(durable) = durable else {
            return CursorWrite::Accepted;
        };
        let (mine_holder, mine_gen) = fence_parts(&self.fence);
        let (their_holder, their_gen) = fence_parts(&durable.fence);
        if !durable.fence.is_empty() {
            if !self.fence.is_empty() && mine_gen < their_gen {
                return CursorWrite::Fenced {
                    actual: durable.fence.clone(),
                    presented: self.fence.clone(),
                };
            }
            if !self.fence.is_empty() && mine_gen == their_gen && mine_holder != their_holder {
                return CursorWrite::Fenced {
                    actual: durable.fence.clone(),
                    presented: self.fence.clone(),
                };
            }
            // An unfenced writer never displaces a fenced row.
            if self.fence.is_empty() {
                return CursorWrite::Fenced {
                    actual: durable.fence.clone(),
                    presented: "(unfenced)".into(),
                };
            }
        }
        // Only an ADVANCE collides with an unacknowledged advance. A rewind or
        // an acknowledgement (`pending = false`) explicitly supersedes it:
        // the earlier position will be re-delivered, so nothing is lost.
        if durable.pending && self.pending {
            let backwards = match (durable.position, self.position) {
                (Some(d), Some(m)) => m < d,
                (Some(_), None) => true,
                _ => false,
            };
            // Re-presenting the SAME unacknowledged advance as a fresh advance
            // would double-count it; an acknowledgement (pending = false) of
            // the same position is exactly what is wanted.
            let duplicate_unacked =
                durable.position == self.position && durable.token == self.token;
            if backwards || duplicate_unacked {
                return CursorWrite::Stale {
                    actual: describe_position(durable),
                    presented: describe_position(self),
                };
            }
        }
        CursorWrite::Accepted
    }

    /// Mark the advance as acknowledged once the caller's work for
    /// `position`/`token` completed. Idempotent; a mismatched position is a
    /// programming error, not a silent no-op.
    pub fn acknowledge(&mut self, position: Option<u64>, token: Option<&str>) -> bool {
        if !self.pending {
            // Nothing outstanding: a second acknowledgement is not a new fact.
            return false;
        }
        if self.position != position {
            return false;
        }
        if let Some(t) = token {
            if self.token.as_deref() != Some(t) {
                return false;
            }
        }
        self.pending = false;
        true
    }
}

/// `position=…/token=…` for audit text.
fn describe_position(c: &FeedCursor) -> String {
    format!(
        "position={} token={}",
        c.position
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".into()),
        c.token.as_deref().unwrap_or("-")
    )
}

impl FeedCursor {
    /// Offer a **sequenced** item. Deterministic:
    ///
    /// * `position <= cursor` → [`CursorAdvance::Duplicate`] (suppressed);
    /// * `position == cursor + 1` (or the first item) → advance;
    /// * `position > cursor + 1` → advance AND report the missing range.
    pub fn offer(
        &mut self,
        position: u64,
        event_at: Option<DateTime<Utc>>,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> CursorAdvance {
        let gap = match self.position {
            Some(last) if position <= last => {
                self.duplicate_count += 1;
                return CursorAdvance::Duplicate;
            }
            Some(last) if position > last + 1 => Some(FeedGap {
                feed: self.feed,
                scope: self.scope.clone(),
                from_position: last + 1,
                to_position: position - 1,
                detected_at: now,
                worker_id: worker_id.to_string(),
                status: GapStatus::Detected,
            }),
            _ => None,
        };
        self.position = Some(position);
        self.last_event_at = event_at.or(self.last_event_at);
        self.updated_at = now;
        self.processed_count += 1;
        self.worker_id = worker_id.to_string();
        // The caller has not done the work yet: persist as unacknowledged so
        // a crash before the side effect replays instead of skipping.
        self.pending = true;
        match gap {
            Some(g) => {
                self.gap_count += 1;
                CursorAdvance::Gap(g)
            }
            None => CursorAdvance::Advanced,
        }
    }

    /// Offer an **opaque** item (signature-style feeds). Ordering cannot be
    /// derived from the token, so only exact repetition of the last token is
    /// a duplicate; feed-level dedup (TASK 2/3 `seen_signatures`,
    /// `copy_event`) remains the authority for older repeats.
    pub fn offer_token(
        &mut self,
        token: &str,
        event_at: Option<DateTime<Utc>>,
        worker_id: &str,
        now: DateTime<Utc>,
    ) -> CursorAdvance {
        if self.token.as_deref() == Some(token) {
            self.duplicate_count += 1;
            return CursorAdvance::Duplicate;
        }
        self.token = Some(token.to_string());
        self.last_event_at = event_at.or(self.last_event_at);
        self.updated_at = now;
        self.processed_count += 1;
        self.worker_id = worker_id.to_string();
        self.pending = true;
        CursorAdvance::Advanced
    }

    /// Rewind a sequenced cursor for a deliberate replay / backfill. The
    /// counters are kept (they are lifetime totals); the caller is expected
    /// to audit the rewind.
    pub fn rewind_to(&mut self, position: Option<u64>, now: DateTime<Utc>) {
        self.position = position;
        // A deliberate rewind is an acknowledged decision, not an unresolved
        // advance.
        self.pending = false;
        self.updated_at = now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cursor() -> FeedCursor {
        FeedCursor::new(FeedId::PolymarketUser, "", "w1")
    }

    #[test]
    fn feed_labels_round_trip() {
        for f in FeedId::ALL {
            assert_eq!(FeedId::parse(f.as_str()), Some(f));
        }
        assert_eq!(FeedId::parse("nope"), None);
        for g in [
            GapStatus::Detected,
            GapStatus::Backfilled,
            GapStatus::Accepted,
        ] {
            assert_eq!(GapStatus::parse(g.as_str()), Some(g));
        }
    }

    #[test]
    fn keys_include_the_scope() {
        let c = FeedCursor::new(FeedId::CopyLogs, "WalletABC", "w1");
        assert_eq!(c.key(), "copy_logs:WalletABC");
        assert_eq!(cursor().key(), "polymarket_user");
    }

    #[test]
    fn sequenced_feed_advances_suppresses_duplicates_and_reports_gaps() {
        let now = Utc::now();
        let mut c = cursor();
        assert_eq!(c.offer(1, Some(now), "w1", now), CursorAdvance::Advanced);
        assert_eq!(c.offer(2, Some(now), "w1", now), CursorAdvance::Advanced);
        // Replay of 2 and of an older one: suppressed, not processed.
        assert_eq!(c.offer(2, Some(now), "w1", now), CursorAdvance::Duplicate);
        assert_eq!(c.offer(1, Some(now), "w1", now), CursorAdvance::Duplicate);
        assert_eq!(c.duplicate_count, 2);
        assert_eq!(c.processed_count, 2);
        // Jump 3,4 missing.
        let adv = c.offer(5, Some(now), "w1", now);
        match &adv {
            CursorAdvance::Gap(g) => {
                assert_eq!((g.from_position, g.to_position), (3, 4));
                assert_eq!(g.len(), 2);
                assert_eq!(g.status, GapStatus::Detected);
                assert!(!g.is_empty());
            }
            other => panic!("expected a gap, got {other:?}"),
        }
        assert!(adv.should_process(), "a gap never drops the current item");
        assert_eq!(c.position, Some(5));
        assert_eq!(c.gap_count, 1);
        assert_eq!(c.processed_count, 3);
        assert_eq!(adv.as_str(), "gap");
    }

    #[test]
    fn opaque_feed_suppresses_only_the_repeated_head() {
        let now = Utc::now();
        let mut c = FeedCursor::new(FeedId::CopyLogs, "wallet", "w1");
        assert_eq!(
            c.offer_token("sig-a", Some(now), "w1", now),
            CursorAdvance::Advanced
        );
        assert_eq!(
            c.offer_token("sig-a", Some(now), "w1", now),
            CursorAdvance::Duplicate
        );
        assert_eq!(
            c.offer_token("sig-b", Some(now), "w1", now),
            CursorAdvance::Advanced
        );
        assert_eq!(c.token.as_deref(), Some("sig-b"));
        assert_eq!(c.processed_count, 2);
        assert_eq!(c.duplicate_count, 1);
    }

    #[test]
    fn restart_resumes_from_the_durable_position() {
        let now = Utc::now();
        let mut c = cursor();
        c.offer(10, Some(now), "w1", now);
        // A new worker loads the same row and continues.
        let mut restored = c.clone();
        assert_eq!(restored.position, Some(10));
        assert_eq!(
            restored.offer(10, Some(now), "w2", now),
            CursorAdvance::Duplicate,
            "the event that was already processed is not processed again"
        );
        assert_eq!(
            restored.offer(11, Some(now), "w2", now),
            CursorAdvance::Advanced
        );
        assert_eq!(restored.worker_id, "w2");
    }

    #[test]
    fn rewind_enables_deliberate_replay() {
        let now = Utc::now();
        let mut c = cursor();
        c.offer(5, Some(now), "w1", now);
        c.rewind_to(Some(2), now);
        assert_eq!(c.position, Some(2));
        assert_eq!(c.offer(3, Some(now), "w1", now), CursorAdvance::Advanced);
        c.rewind_to(None, now);
        assert_eq!(c.offer(100, Some(now), "w1", now), CursorAdvance::Advanced);
    }

    /// The admission matrix. Every row is a case that used to be a blind
    /// "last write wins" upsert.
    #[test]
    fn cursor_writes_are_fenced_and_replay_safe() {
        let now = Utc::now();
        let base = |fence: &str, position: u64, pending: bool| {
            let mut c = FeedCursor::new(FeedId::PolymarketUser, "", "w");
            c.fence = fence.into();
            c.offer(position, None, "w", now);
            c.pending = pending;
            c
        };

        // Absent durable row: accepted.
        assert_eq!(
            base("w-a@1", 1, true).admit_write(None),
            CursorWrite::Accepted
        );

        // A stale generation of any holder loses, whatever its position.
        let durable = base("w-b@2", 10, false);
        match base("w-a@1", 99, true).admit_write(Some(&durable)) {
            CursorWrite::Fenced {
                actual, presented, ..
            } => {
                assert_eq!(actual, "w-b@2");
                assert_eq!(presented, "w-a@1");
            }
            other => panic!("expected fenced, got {other:?}"),
        }

        // Two holders at the same generation: split brain, no winner.
        assert!(matches!(
            base("w-a@2", 11, true).admit_write(Some(&durable)),
            CursorWrite::Fenced { .. }
        ));

        // An unfenced writer (no cluster leases) never displaces a fenced row.
        assert!(matches!(
            base("", 11, true).admit_write(Some(&durable)),
            CursorWrite::Fenced { .. }
        ));

        // A newer generation of the same holder advances normally.
        assert_eq!(
            base("w-b@3", 11, true).admit_write(Some(&durable)),
            CursorWrite::Accepted
        );

        // An unacknowledged advance is never overwritten backwards…
        let pending = base("w-b@2", 10, true);
        assert!(matches!(
            base("w-b@2", 4, true).admit_write(Some(&pending)),
            CursorWrite::Stale { .. }
        ));
        // …nor re-offered as if it were new (that would double-count)…
        assert!(matches!(
            base("w-b@2", 10, true).admit_write(Some(&pending)),
            CursorWrite::Stale { .. }
        ));
        // …but acknowledging it, or a deliberate rewind, is exactly right.
        assert_eq!(
            base("w-b@2", 10, false).admit_write(Some(&pending)),
            CursorWrite::Accepted
        );
        let mut rewind = base("w-b@2", 10, false);
        rewind.rewind_to(Some(2), now);
        assert_eq!(rewind.admit_write(Some(&pending)), CursorWrite::Accepted);

        // Acknowledged rows take any forward position.
        let acked = base("w-b@2", 10, false);
        assert_eq!(
            base("w-b@2", 40, true).admit_write(Some(&acked)),
            CursorWrite::Accepted
        );
        // An unparseable (generation-less) fence sorts as generation 0, so any
        // real generation may take the row — but it can never take one from a
        // real fence, which is the case asserted above.
        let odd = base("garbage", 1, false);
        assert_eq!(
            base("w-b@1", 2, true).admit_write(Some(&odd)),
            CursorWrite::Accepted
        );
    }

    #[test]
    fn an_advance_is_pending_until_acknowledged() {
        let now = Utc::now();
        let mut c = cursor();
        assert!(!c.pending);
        assert_eq!(c.offer(1, None, "w1", now), CursorAdvance::Advanced);
        assert!(c.pending, "the caller has not done the work yet");
        assert!(CursorAdvance::Advanced.needs_ack());
        assert!(!CursorAdvance::Duplicate.needs_ack());
        assert!(!c.acknowledge(Some(2), None), "wrong position never acks");
        assert!(c.acknowledge(Some(1), None));
        assert!(!c.pending);
        assert!(
            !c.acknowledge(Some(1), None),
            "acknowledging twice is not a new fact"
        );
        assert!(!CursorAdvance::Refused {
            reason: "fenced".into(),
            detail: String::new()
        }
        .should_process());
    }

    #[test]
    fn lag_is_measured_from_the_last_event() {
        let now = Utc::now();
        let mut c = cursor();
        assert_eq!(c.lag_secs(now), None);
        c.offer(1, Some(now - chrono::Duration::seconds(12)), "w1", now);
        assert_eq!(c.lag_secs(now), Some(12));
    }
}
