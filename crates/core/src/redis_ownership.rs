//! Redis-backed execution-ownership store (Prompt 3 §D/§U) — used ONLY when
//! Postgres is absent (per the store-authority order Postgres > Redis >
//! Memory in [`crate::ownership`]).
//!
//! Key namespace (§U): `own:claim:{execution_id}` — one hash per logical
//! execution:
//!
//! ```text
//! owner, epoch, status ('claimed'|'released'|'handed_off'),
//! claimed_at, lease_until, lease_span, updated_at   (unix ms, Redis TIME)
//! takeovers, prev_owner, kind, module, strategy, symbol
//! ```
//!
//! All state transitions are single Lua scripts (atomic under Redis's
//! single-threaded execution — the same technique as the existing dedup/lock
//! primitives), and ALL timestamps come from `redis.call('TIME')` so
//! replicas with skewed local clocks cannot manufacture or extend leases.
//!
//! Durability stance (§K + the workspace Redis rule): a claim is a LEASE,
//! not money state. If Redis restarts, hashes vanish → every active claim
//! reads as absent → a replica may re-acquire with epoch 1 — which is why
//! the intent journal + reconciliation remain the backstop for in-flight
//! ambiguity, and why Postgres is the authoritative store whenever
//! configured. Terminal (`released`/`handed_off`) hashes are kept 7 days
//! for observability, then expire; Postgres rows are never deleted.
//!
//! Fail-closed: every method propagates Redis errors/timeouts as `Err` —
//! [`crate::ownership::OwnershipRegistry`] turns them into
//! `OwnershipUnavailable` and money paths abort.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use once_cell::sync::Lazy;
use redis::{Script, Value};
use tracing::debug;

use crate::error::{BotError, BotResult};
use crate::ownership::{
    ClaimDecision, ClaimRequest, ClaimStatus, ClaimStore, ExecutionClaim, RuntimeFlag,
    RuntimeFlagsReader, RuntimeFlagsWriter, StoreContext,
};
use crate::redis_kv::RedisKv;

/// Terminal-state retention in Redis (audit lives in Postgres/logs).
const TERMINAL_TTL_MS: i64 = 7 * 24 * 3600 * 1000;

fn claim_key(execution_id: &str) -> String {
    format!("own:claim:{execution_id}")
}

fn flag_key(flag: &str) -> String {
    format!("own:flag:{flag}")
}

/// Atomic acquire/takeover. Returns
/// `(acquired, epoch, status, lease_until_ms, takeovers,
/// prev_owner_or_current_owner, claimed_or_updated_ms, reacq)` where
/// `reacq` is 0 = fresh, 1 = re-acquisition (released / post-grace),
/// 2 = TAKEOVER of an expired lease (metered on the Rust side).
static CLAIM: Lazy<Script> = Lazy::new(|| {
    Script::new(
        r#"
local t = redis.call('TIME')
local now_ms = t[1] * 1000 + math.floor(t[2] / 1000)
local lease_ms = tonumber(ARGV[1])
local grace_ms = tonumber(ARGV[2])
local owner = ARGV[3]
local function stamp(status, epoch, takeovers, prev, reacq)
    redis.call('HSET', KEYS[1],
        'owner', owner, 'epoch', epoch, 'status', status,
        'claimed_at', now_ms, 'lease_until', now_ms + lease_ms,
        'lease_span', lease_ms, 'updated_at', now_ms,
        'takeovers', takeovers, 'prev_owner', prev,
        'kind', ARGV[4], 'module', ARGV[5], 'strategy', ARGV[6], 'symbol', ARGV[7])
    return {1, epoch, status, now_ms + lease_ms, takeovers, prev, now_ms, reacq}
end
if redis.call('EXISTS', KEYS[1]) == 0 then
    local out = stamp('claimed', 1, 0, '', 0)
    redis.call('PEXPIRE', KEYS[1], lease_ms * 2 + grace_ms)
    return out
end
local status = redis.call('HGET', KEYS[1], 'status')
local lease_until = tonumber(redis.call('HGET', KEYS[1], 'lease_until')) or 0
local updated = tonumber(redis.call('HGET', KEYS[1], 'updated_at')) or 0
local reacq = 0
if status == 'released' then
    reacq = 1
elseif status == 'claimed' and lease_until <= now_ms then
    reacq = 2
elseif status == 'handed_off' and updated + grace_ms <= now_ms then
    reacq = 1
end
if reacq == 0 then
    local cur_owner = redis.call('HGET', KEYS[1], 'owner') or ''
    local cur_epoch = tonumber(redis.call('HGET', KEYS[1], 'epoch')) or 0
    return {0, cur_epoch, status, lease_until, 0, cur_owner, updated, 0}
end
local old_owner = redis.call('HGET', KEYS[1], 'owner') or ''
local epoch = (tonumber(redis.call('HGET', KEYS[1], 'epoch')) or 0) + 1
local takeovers = tonumber(redis.call('HGET', KEYS[1], 'takeovers')) or 0
if reacq == 2 then
    takeovers = takeovers + 1
end
local out = stamp('claimed', epoch, takeovers, old_owner, reacq)
redis.call('PEXPIRE', KEYS[1], lease_ms * 2 + grace_ms)
return out
"#,
    )
});

/// CAS renew on (owner, epoch, claimed, unexpired). Extends by the lease
/// span recorded at claim time. Returns 0/1.
static RENEW: Lazy<Script> = Lazy::new(|| {
    Script::new(
        r#"
local t = redis.call('TIME')
local now_ms = t[1] * 1000 + math.floor(t[2] / 1000)
if redis.call('HGET', KEYS[1], 'owner') ~= ARGV[1] then return 0 end
if tonumber(redis.call('HGET', KEYS[1], 'epoch')) ~= tonumber(ARGV[2]) then return 0 end
if redis.call('HGET', KEYS[1], 'status') ~= 'claimed' then return 0 end
if (tonumber(redis.call('HGET', KEYS[1], 'lease_until')) or 0) <= now_ms then return 0 end
local span = tonumber(redis.call('HGET', KEYS[1], 'lease_span')) or 45000
redis.call('HSET', KEYS[1], 'lease_until', now_ms + span, 'updated_at', now_ms)
return 1
"#,
    )
});

/// Fencing check: is (owner, epoch) the active, unexpired holder? 0/1.
static VERIFY: Lazy<Script> = Lazy::new(|| {
    Script::new(
        r#"
local t = redis.call('TIME')
local now_ms = t[1] * 1000 + math.floor(t[2] / 1000)
if redis.call('HGET', KEYS[1], 'owner') ~= ARGV[1] then return 0 end
if tonumber(redis.call('HGET', KEYS[1], 'epoch')) ~= tonumber(ARGV[2]) then return 0 end
if redis.call('HGET', KEYS[1], 'status') ~= 'claimed' then return 0 end
if (tonumber(redis.call('HGET', KEYS[1], 'lease_until')) or 0) <= now_ms then return 0 end
return 1
"#,
    )
});

/// CAS terminal transition (released | handed_off). 0/1.
static RELEASE: Lazy<Script> = Lazy::new(|| {
    Script::new(
        r#"
local t = redis.call('TIME')
local now_ms = t[1] * 1000 + math.floor(t[2] / 1000)
if redis.call('HGET', KEYS[1], 'owner') ~= ARGV[1] then return 0 end
if tonumber(redis.call('HGET', KEYS[1], 'epoch')) ~= tonumber(ARGV[2]) then return 0 end
if redis.call('HGET', KEYS[1], 'status') ~= 'claimed' then return 0 end
redis.call('HSET', KEYS[1], 'status', ARGV[3], 'updated_at', now_ms)
redis.call('PEXPIRE', KEYS[1], ARGV[4])
return 1
"#,
    )
});

fn ms_to_dt(ms: i64) -> DateTime<Utc> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Utc::now)
}

fn value_to_i64(v: &Value) -> i64 {
    match v {
        Value::Int(i) => *i,
        Value::SimpleString(s) => s.parse().unwrap_or(0),
        Value::BulkString(b) => String::from_utf8_lossy(b).parse().unwrap_or(0),
        _ => 0,
    }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::SimpleString(s) => s.clone(),
        Value::BulkString(b) => String::from_utf8_lossy(b).into_owned(),
        Value::Int(i) => i.to_string(),
        Value::Nil => String::new(),
        _ => String::new(),
    }
}

/// Redis [`ClaimStore`] (`own:claim:{execution_id}` hashes + Lua CAS).
pub struct RedisClaimStore {
    kv: RedisKv,
}

impl RedisClaimStore {
    pub fn new(kv: RedisKv) -> Self {
        RedisClaimStore { kv }
    }
}

#[async_trait]
impl ClaimStore for RedisClaimStore {
    fn backend(&self) -> &'static str {
        "redis"
    }

    async fn claim(&self, req: &ClaimRequest, ctx: &StoreContext) -> BotResult<ClaimDecision> {
        let key = claim_key(&req.execution_id);
        let lease_ms = ctx.lease.as_millis().max(1000) as i64;
        let grace_ms = ctx.handoff_grace.as_millis().max(1000) as i64;
        let (owner, kind, module, strategy, symbol) = (
            ctx.owner_id.clone(),
            req.kind.clone(),
            req.module.clone(),
            req.strategy.clone(),
            req.symbol.clone(),
        );
        let out: Vec<Value> = self
            .kv
            .op("claim_acquire", move |mut c| {
                let key = key.clone();
                async move {
                    let v = CLAIM
                        .key(&key)
                        .arg(lease_ms)
                        .arg(grace_ms)
                        .arg(&owner)
                        .arg(&kind)
                        .arg(&module)
                        .arg(&strategy)
                        .arg(&symbol)
                        .invoke_async(&mut c)
                        .await?;
                    Ok(v)
                }
            })
            .await?;
        if out.len() < 8 {
            return Err(BotError::db(format!(
                "claim script returned {} fields (expected 8)",
                out.len()
            )));
        }
        let acquired = value_to_i64(&out[0]);
        let epoch = value_to_i64(&out[1]);
        let status_s = value_to_string(&out[2]);
        let lease_until_ms = value_to_i64(&out[3]);
        let takeovers = value_to_i64(&out[4]);
        let other_owner = value_to_string(&out[5]);
        let ts_ms = value_to_i64(&out[6]);
        let reacq = value_to_i64(&out[7]);
        if reacq == 2 {
            crate::obs::metrics::global()
                .counter(
                    "bot_distributed_claim_expired_total",
                    "Claims found expired and taken over by a new replica.",
                    &[("module", req.module.as_str())],
                )
                .inc();
            crate::obs::metrics::global()
                .counter(
                    "bot_distributed_claim_takeover_total",
                    "Takeovers of expired claims by a new replica.",
                    &[("module", req.module.as_str())],
                )
                .inc();
        }
        if acquired == 1 {
            Ok(ClaimDecision::Acquired(ExecutionClaim {
                execution_id: req.execution_id.clone(),
                kind: req.kind.clone(),
                module: req.module.clone(),
                strategy: req.strategy.clone(),
                symbol: req.symbol.clone(),
                owner_id: ctx.owner_id.clone(),
                epoch,
                status: ClaimStatus::Claimed,
                acquired_at: ms_to_dt(ts_ms),
                lease_until: ms_to_dt(lease_until_ms),
                takeover_count: takeovers,
                previous_owner: if other_owner.is_empty() {
                    None
                } else {
                    Some(other_owner)
                },
            }))
        } else {
            Ok(ClaimDecision::Rejected {
                owner_id: other_owner,
                epoch,
                status: ClaimStatus::parse(&status_s).unwrap_or(ClaimStatus::Claimed),
                lease_until: ms_to_dt(lease_until_ms),
            })
        }
    }

    async fn renew(&self, execution_id: &str, owner_id: &str, epoch: i64) -> BotResult<bool> {
        let key = claim_key(execution_id);
        let (owner_id, epoch) = (owner_id.to_string(), epoch);
        let n: i64 = self
            .kv
            .op("claim_renew", move |mut c| {
                let key = key.clone();
                async move {
                    let v = RENEW
                        .key(&key)
                        .arg(&owner_id)
                        .arg(epoch)
                        .invoke_async(&mut c)
                        .await?;
                    Ok(v)
                }
            })
            .await?;
        Ok(n == 1)
    }

    async fn verify(&self, execution_id: &str, owner_id: &str, epoch: i64) -> BotResult<bool> {
        let key = claim_key(execution_id);
        let (owner_id, epoch) = (owner_id.to_string(), epoch);
        let n: i64 = self
            .kv
            .op("claim_verify", move |mut c| {
                let key = key.clone();
                async move {
                    let v = VERIFY
                        .key(&key)
                        .arg(&owner_id)
                        .arg(epoch)
                        .invoke_async(&mut c)
                        .await?;
                    Ok(v)
                }
            })
            .await?;
        Ok(n == 1)
    }

    async fn release(
        &self,
        execution_id: &str,
        owner_id: &str,
        epoch: i64,
        mode: ClaimStatus,
    ) -> BotResult<bool> {
        debug_assert!(mode != ClaimStatus::Claimed);
        let key = claim_key(execution_id);
        let (owner_id, epoch, mode_s) = (owner_id.to_string(), epoch, mode.as_str().to_string());
        let n: i64 = self
            .kv
            .op("claim_release", move |mut c| {
                let key = key.clone();
                async move {
                    let v = RELEASE
                        .key(&key)
                        .arg(&owner_id)
                        .arg(epoch)
                        .arg(&mode_s)
                        .arg(TERMINAL_TTL_MS)
                        .invoke_async(&mut c)
                        .await?;
                    Ok(v)
                }
            })
            .await?;
        Ok(n == 1)
    }

    async fn get(&self, execution_id: &str) -> BotResult<Option<ExecutionClaim>> {
        let key = claim_key(execution_id);
        let fields: HashMap<String, String> = self
            .kv
            .op("claim_get", move |mut c| {
                let key = key.clone();
                async move {
                    let v: HashMap<String, String> =
                        redis::cmd("HGETALL").arg(&key).query_async(&mut c).await?;
                    Ok(v)
                }
            })
            .await?;
        if fields.is_empty() {
            return Ok(None);
        }
        let get_i = |k: &str| {
            fields
                .get(k)
                .and_then(|v| v.parse::<i64>().ok())
                .unwrap_or(0)
        };
        Ok(Some(ExecutionClaim {
            execution_id: execution_id.to_string(),
            kind: fields.get("kind").cloned().unwrap_or_default(),
            module: fields.get("module").cloned().unwrap_or_default(),
            strategy: fields.get("strategy").cloned().unwrap_or_default(),
            symbol: fields.get("symbol").cloned().unwrap_or_default(),
            owner_id: fields.get("owner").cloned().unwrap_or_default(),
            epoch: get_i("epoch").max(1),
            status: ClaimStatus::parse(fields.get("status").map(|s| s.as_str()).unwrap_or(""))
                .unwrap_or(ClaimStatus::Claimed),
            acquired_at: ms_to_dt(get_i("claimed_at")),
            lease_until: ms_to_dt(get_i("lease_until")),
            takeover_count: get_i("takeovers"),
            previous_owner: fields.get("prev_owner").filter(|s| !s.is_empty()).cloned(),
        }))
    }
}

/// Redis runtime flags (§Q fallback when no Postgres): `own:flag:{flag}`
/// hashes, no TTL (flags are control state, tiny, and must survive restarts
/// as long as Redis itself does).
pub struct RedisFlags {
    kv: RedisKv,
}

impl RedisFlags {
    pub fn new(kv: RedisKv) -> Self {
        RedisFlags { kv }
    }
}

#[async_trait]
impl RuntimeFlagsWriter for RedisFlags {
    async fn write(&self, flag: &str, enabled: bool, reason: &str, updated_by: &str) {
        let key = flag_key(flag);
        let (enabled_s, reason, updated_by) = (
            if enabled { "1" } else { "0" },
            reason.to_string(),
            updated_by.to_string(),
        );
        let now_ms = Utc::now().timestamp_millis();
        let res: BotResult<()> = self
            .kv
            .op("flags_write", move |mut c| {
                let key = key.clone();
                async move {
                    let _: Value = redis::cmd("HSET")
                        .arg(&key)
                        .arg("enabled")
                        .arg(enabled_s)
                        .arg("reason")
                        .arg(&reason)
                        .arg("updated_by")
                        .arg(&updated_by)
                        .arg("updated_at")
                        .arg(now_ms)
                        .query_async(&mut c)
                        .await?;
                    Ok(())
                }
            })
            .await;
        if let Err(e) = res {
            tracing::warn!(flag, enabled, error = %e, "redis runtime flag publish FAILED");
        }
    }
}

#[async_trait]
impl RuntimeFlagsReader for RedisFlags {
    async fn read_all(&self) -> BotResult<Vec<RuntimeFlag>> {
        // SCAN (never KEYS) + HGETALL per flag. Flag cardinality is tiny
        // (kill_switch + ≤5 modules).
        let mut cursor: u64 = 0;
        let mut out = Vec::new();
        loop {
            let cur = cursor;
            let (next, keys): (u64, Vec<String>) = self
                .kv
                .op("flags_scan", move |mut c| async move {
                    let v: (u64, Vec<String>) = redis::cmd("SCAN")
                        .arg(cur)
                        .arg("MATCH")
                        .arg("own:flag:*")
                        .arg("COUNT")
                        .arg(100)
                        .query_async(&mut c)
                        .await?;
                    Ok(v)
                })
                .await?;
            cursor = next;
            for key in keys {
                let k = key.clone();
                let fields: HashMap<String, String> = self
                    .kv
                    .op("flags_read", move |mut c| {
                        let k = k.clone();
                        async move {
                            let v: HashMap<String, String> =
                                redis::cmd("HGETALL").arg(&k).query_async(&mut c).await?;
                            Ok(v)
                        }
                    })
                    .await?;
                let flag = key.strip_prefix("own:flag:").unwrap_or(&key).to_string();
                let updated_ms = fields
                    .get("updated_at")
                    .and_then(|v| v.parse::<i64>().ok())
                    .unwrap_or(0);
                out.push(RuntimeFlag {
                    flag,
                    enabled: fields.get("enabled").map(|v| v == "1").unwrap_or(false),
                    reason: fields.get("reason").cloned().unwrap_or_default(),
                    updated_by: fields.get("updated_by").cloned().unwrap_or_default(),
                    updated_at: ms_to_dt(updated_ms),
                });
            }
            if cursor == 0 {
                break;
            }
        }
        debug!(flags = out.len(), "redis runtime flags read");
        Ok(out)
    }
}

/// Convenience constructor used by the server bootstrap: `Arc<dyn ClaimStore>`.
pub fn redis_claim_store(kv: RedisKv) -> Arc<dyn ClaimStore> {
    Arc::new(RedisClaimStore::new(kv))
}
