//! `/consent grant|status|revoke`.
//!
//! The handlers here only *orchestrate*: they resolve the chat link, ask
//! [`crate::consent::policy`] whether a consent may exist, ask the store to
//! persist it, cancel anything that must die, write one audit row, and let
//! [`crate::commands::reply`] phrase the answer. No rule is duplicated.

use chrono::{DateTime, Utc};

use crate::commands::audit::{AuditDetail, AuditEvent};
use crate::commands::parse::{self, ParseError};
use crate::commands::reply::{self, Reply};
use crate::commands::TradeConsole;
use crate::consent::policy::{self, ConsentBounds, ConsentRequest, GrantContext};
use crate::consent::store::StoreError;
use crate::consent::{Consent, ConsentError};

/// The `/consent` sub-command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConsentAction {
    /// `/consent grant [max_order_sol] [daily_cap_sol] [ttl_hours] <phrase>`.
    Grant {
        /// The raw arguments after `grant` (validated by
        /// [`crate::commands::parse`] and [`crate::consent::policy`]).
        args: Vec<String>,
    },
    /// `/consent status`.
    Status,
    /// `/consent revoke`.
    Revoke,
    /// `/consent help` — the command reference.
    Usage,
    /// An unrecognized sub-command (refused, with the usage text).
    Invalid(String),
}

impl ConsentAction {
    /// Parse everything after the `consent` token.
    pub fn parse(rest: &str) -> Self {
        let mut tokens = rest.split_whitespace();
        let Some(sub) = tokens.next() else {
            // `/consent` with no sub-command shows usage instead of guessing.
            return ConsentAction::Usage;
        };
        let tail: Vec<String> = tokens.map(str::to_string).collect();
        match sub.to_ascii_lowercase().as_str() {
            "grant" => ConsentAction::Grant { args: tail },
            "status" | "show" => ConsentAction::Status,
            "revoke" | "off" | "stop" => ConsentAction::Revoke,
            "help" => ConsentAction::Usage,
            other => ConsentAction::Invalid(other.to_string()),
        }
    }

    /// Stable label for audit rows and reply text.
    pub fn as_str(&self) -> &'static str {
        match self {
            ConsentAction::Grant { .. } => "grant",
            ConsentAction::Status => "status",
            ConsentAction::Revoke => "revoke",
            ConsentAction::Usage => "help",
            ConsentAction::Invalid(_) => "invalid",
        }
    }
}

impl TradeConsole {
    /// Route one `/consent` sub-command.
    pub(crate) async fn handle_consent(
        &self,
        action: ConsentAction,
        chat_id: i64,
        user_id: Option<i64>,
        now: DateTime<Utc>,
    ) -> Reply {
        match action {
            ConsentAction::Grant { args } => self.consent_grant(chat_id, user_id, &args, now).await,
            ConsentAction::Status => self.consent_status(chat_id, user_id, now).await,
            ConsentAction::Revoke => self.consent_revoke(chat_id, user_id, now).await,
            ConsentAction::Usage => reply::trade_help(),
            ConsentAction::Invalid(sub) => reply::denied_with_hint(
                "unknown_consent_command",
                &format!("'{sub}' is not a /consent sub-command"),
                "Use /consent grant, /consent status or /consent revoke.",
            ),
        }
    }

    /// `/consent grant …` — creates the capability, after the typed phrase.
    async fn consent_grant(
        &self,
        chat_id: i64,
        user_id: Option<i64>,
        args: &[String],
        now: DateTime<Utc>,
    ) -> Reply {
        let Some(user_id) = user_id else {
            return self
                .deny_unlinked(chat_id, "unlinked_chat", "this message has no sender", now)
                .await;
        };
        let link = match self.resolve_link(chat_id, user_id).await {
            Ok(link) => link,
            Err(deny) => return self.deny_link(chat_id, None, deny, now).await,
        };
        // Caps must be checked against the organization's OWN risk limits, so a
        // consent can never widen them. Without those limits we cannot grant.
        let org_limits = match self.port.org_risk_limits(&link.organization_id).await {
            Ok(limits) => limits,
            Err(err) => {
                return self
                    .deny(
                        Some(&link.organization_id),
                        chat_id,
                        err.code(),
                        err.detail(),
                        now,
                    )
                    .await
            }
        };
        let parsed = match parse::parse_consent_grant(args, &self.consent_bounds) {
            Ok(parsed) => parsed,
            Err(err) => {
                return self
                    .deny(
                        Some(&link.organization_id),
                        chat_id,
                        err.code(),
                        &err.message(),
                        now,
                    )
                    .await
            }
        };
        let request = ConsentRequest {
            order_lamports: parsed.order_lamports,
            daily_lamports: parsed.daily_lamports,
            ttl_hours: parsed.ttl_hours,
        };
        let ctx = GrantContext {
            organization_id: &link.organization_id,
            user_id,
            chat_id,
            now,
            org_limits,
            phrase: &parsed.phrase,
        };
        let consent = match policy::draft_consent(&self.consent_bounds, &request, &ctx) {
            Ok(consent) => consent,
            Err(ConsentError::PhraseMismatch) => {
                return self
                    .deny(
                        Some(&link.organization_id),
                        chat_id,
                        ConsentError::PhraseMismatch.code(),
                        &format!(
                            "the confirmation phrase must be typed exactly: {}",
                            policy::CONSENT_PHRASE
                        ),
                        now,
                    )
                    .await
            }
            Err(err) => {
                return self
                    .deny(
                        Some(&link.organization_id),
                        chat_id,
                        err.code(),
                        &describe_consent_error(&err),
                        now,
                    )
                    .await
            }
        };
        if let Err(err) = self.consent_store.insert(&consent).await {
            let code = match &err {
                StoreError::Conflict(_) => ConsentError::AlreadyActive.code(),
                StoreError::Unavailable(_) => "consent_store_unavailable",
            };
            return self
                .deny(
                    Some(&link.organization_id),
                    chat_id,
                    code,
                    err.detail(),
                    now,
                )
                .await;
        }
        // Granting without an audit trail is not acceptable: if the ledger
        // cannot record it, the consent is withdrawn again and the request is
        // refused.
        if let Err(err) = self
            .audit(
                Some(consent.consent_id),
                Some(&link.organization_id),
                chat_id,
                AuditEvent::ConsentGranted,
                AuditDetail::new()
                    .lamports(
                        crate::commands::audit::AuditAmount::OrderCapLamports,
                        consent.max_order_lamports,
                    )
                    .lamports(
                        crate::commands::audit::AuditAmount::DailyCapLamports,
                        consent.daily_cap_lamports,
                    )
                    .mode("grant"),
                now,
            )
            .await
        {
            let _ = self
                .consent_store
                .revoke(&link.organization_id, chat_id, now)
                .await;
            return self
                .deny(
                    Some(&link.organization_id),
                    chat_id,
                    "audit_store_unavailable",
                    err.detail(),
                    now,
                )
                .await;
        }
        reply::consent_granted(
            &link.organization_id,
            consent.max_order_lamports,
            consent.daily_cap_lamports,
            &consent.expires_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            consent.seconds_remaining(now) / 60,
        )
    }

    /// `/consent status` — read-only.
    async fn consent_status(
        &self,
        chat_id: i64,
        user_id: Option<i64>,
        now: DateTime<Utc>,
    ) -> Reply {
        let Some(user_id) = user_id else {
            return self
                .deny_unlinked(chat_id, "unlinked_chat", "this message has no sender", now)
                .await;
        };
        let link = match self.resolve_link(chat_id, user_id).await {
            Ok(link) => link,
            Err(deny) => return self.deny_link(chat_id, None, deny, now).await,
        };
        let consent = match self
            .consent_store
            .active(&link.organization_id, chat_id, now)
            .await
        {
            Ok(consent) => consent,
            Err(err) => {
                return self
                    .deny(
                        Some(&link.organization_id),
                        chat_id,
                        "consent_store_unavailable",
                        err.detail(),
                        now,
                    )
                    .await
            }
        };
        match consent {
            None => reply::consent_missing(),
            Some(consent) => describe_consent(&link.organization_id, &consent, now),
        }
    }

    /// `/consent revoke` — immediate, and cancels pending commands.
    async fn consent_revoke(
        &self,
        chat_id: i64,
        user_id: Option<i64>,
        now: DateTime<Utc>,
    ) -> Reply {
        let Some(user_id) = user_id else {
            return self
                .deny_unlinked(chat_id, "unlinked_chat", "this message has no sender", now)
                .await;
        };
        let link = match self.resolve_link(chat_id, user_id).await {
            Ok(link) => link,
            Err(deny) => return self.deny_link(chat_id, None, deny, now).await,
        };
        let revoked = match self
            .consent_store
            .revoke(&link.organization_id, chat_id, now)
            .await
        {
            Ok(revoked) => revoked,
            Err(err) => {
                return self
                    .deny(
                        Some(&link.organization_id),
                        chat_id,
                        "consent_store_unavailable",
                        err.detail(),
                        now,
                    )
                    .await
            }
        };
        let Some(consent) = revoked else {
            return reply::consent_revoke_nothing();
        };
        // Pending commands die with the consent: an unconfirmed nonce must not
        // survive a revocation.
        let cancelled = match self
            .command_store
            .cancel_pending(&link.organization_id, chat_id, now)
            .await
        {
            Ok(ids) => ids,
            Err(err) => {
                return self
                    .deny(
                        Some(&link.organization_id),
                        chat_id,
                        "command_store_unavailable",
                        err.detail(),
                        now,
                    )
                    .await
            }
        };
        for command_id in &cancelled {
            let _ = self
                .audit(
                    Some(*command_id),
                    Some(&link.organization_id),
                    chat_id,
                    AuditEvent::Cancelled,
                    AuditDetail::new().reason("consent_revoked"),
                    now,
                )
                .await;
        }
        let affected = cancelled.len() as u64;
        if let Err(err) = self
            .audit(
                Some(consent.consent_id),
                Some(&link.organization_id),
                chat_id,
                AuditEvent::Revoked,
                AuditDetail::new().affected(affected),
                now,
            )
            .await
        {
            // The revocation itself already happened in the store; keep the
            // deterministic reply but surface the audit failure in the log.
            tracing::warn!(
                chat_id,
                error = %err.detail(),
                "consent revoked but the audit row could not be written"
            );
        }
        reply::consent_revoked(affected)
    }
}

/// Render a live consent for `/consent status`.
fn describe_consent(organization_id: &str, consent: &Consent, now: DateTime<Utc>) -> Reply {
    let status = consent.status(now);
    let scope = consent.scope.labels().join(", ");
    reply::consent_status(
        status.as_str(),
        organization_id,
        &scope,
        consent.max_order_lamports,
        consent.daily_cap_lamports,
        consent.seconds_remaining(now),
        status == crate::consent::ConsentStatus::Revoked,
    )
}

/// Human explanation for a consent rule failure (never contains a secret).
fn describe_consent_error(error: &ConsentError) -> String {
    match error {
        ConsentError::PhraseMismatch => "the confirmation phrase did not match".to_string(),
        ConsentError::EmptyScope => "the consent scope would be empty".to_string(),
        ConsentError::UnknownScopeLabel(label) => format!("unknown scope '{label}'"),
        ConsentError::OrderCapZero => "the per-order cap must be greater than zero".to_string(),
        ConsentError::DailyCapBelowOrder => {
            "the 24h cap must be at least the per-order cap".to_string()
        }
        ConsentError::ExceedsOrgOrderLimit { requested, ceiling } => format!(
            "the per-order cap {requested} lamports exceeds the organization limit of {ceiling} lamports"
        ),
        ConsentError::ExceedsOrgDailyLimit { requested, ceiling } => format!(
            "the 24h cap {requested} lamports exceeds the organization limit of {ceiling} lamports"
        ),
        ConsentError::TtlOutOfRange { requested, min, max } => {
            format!("a TTL of {requested}h is outside {min}..{max}h")
        }
        ConsentError::Missing => "no consent is on file for this chat".to_string(),
        ConsentError::Expired => "the consent expired".to_string(),
        ConsentError::Revoked => "the consent was revoked".to_string(),
        ConsentError::ScopeDenied(action) => {
            format!("the consent does not cover {}", action.as_str())
        }
        ConsentError::AlreadyActive => {
            "a live consent already exists for this chat — /consent revoke it first".to_string()
        }
        ConsentError::StoreUnavailable(detail) => detail.clone(),
    }
}

/// Exposed for the integration tests: the exact phrase an operator must type.
pub fn phrase_hint() -> &'static str {
    policy::CONSENT_PHRASE
}

/// The consent bounds the console validates against (re-exported so a caller
/// does not have to reach into two modules).
pub fn bounds() -> ConsentBounds {
    ConsentBounds::default()
}

/// Convenience: parse a grant argument list the way the dispatcher does.
pub fn parse_grant_args(args: &[String]) -> Result<parse::GrantArgs, ParseError> {
    parse::parse_consent_grant(args, &bounds())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subcommands_parse() {
        assert_eq!(ConsentAction::parse("status"), ConsentAction::Status);
        assert_eq!(ConsentAction::parse("STATUS"), ConsentAction::Status);
        assert_eq!(ConsentAction::parse("revoke"), ConsentAction::Revoke);
        assert_eq!(ConsentAction::parse("help"), ConsentAction::Usage);
        assert_eq!(ConsentAction::parse(""), ConsentAction::Usage);
        assert_eq!(
            ConsentAction::parse("grant 0.05 0.2 24 I CONSENT TO TELEGRAM TRADING"),
            ConsentAction::Grant {
                args: vec![
                    "0.05".to_string(),
                    "0.2".to_string(),
                    "24".to_string(),
                    "I".to_string(),
                    "CONSENT".to_string(),
                    "TO".to_string(),
                    "TELEGRAM".to_string(),
                    "TRADING".to_string(),
                ]
            }
        );
        assert_eq!(
            ConsentAction::parse("frobnicate"),
            ConsentAction::Invalid("frobnicate".into())
        );
        assert_eq!(ConsentAction::Status.as_str(), "status");
        assert_eq!(ConsentAction::Grant { args: vec![] }.as_str(), "grant");
        assert_eq!(ConsentAction::Revoke.as_str(), "revoke");
        assert_eq!(ConsentAction::Usage.as_str(), "help");
        assert_eq!(ConsentAction::Invalid("x".into()).as_str(), "invalid");
    }

    #[test]
    fn consent_errors_are_explained() {
        for error in [
            ConsentError::PhraseMismatch,
            ConsentError::EmptyScope,
            ConsentError::UnknownScopeLabel("x".into()),
            ConsentError::OrderCapZero,
            ConsentError::DailyCapBelowOrder,
            ConsentError::ExceedsOrgOrderLimit {
                requested: 2,
                ceiling: 1,
            },
            ConsentError::ExceedsOrgDailyLimit {
                requested: 2,
                ceiling: 1,
            },
            ConsentError::TtlOutOfRange {
                requested: 0,
                min: 1,
                max: 2,
            },
            ConsentError::Missing,
            ConsentError::Expired,
            ConsentError::Revoked,
            ConsentError::ScopeDenied(crate::trade_port::TradeAction::Buy),
            ConsentError::AlreadyActive,
            ConsentError::StoreUnavailable("db".into()),
        ] {
            let text = describe_consent_error(&error);
            assert!(!text.is_empty(), "{error:?}");
            assert!(text.len() < 200, "{text}");
        }
    }

    #[test]
    fn phrase_hint_and_bounds_are_the_policy_values() {
        assert_eq!(phrase_hint(), policy::CONSENT_PHRASE);
        assert_eq!(bounds(), ConsentBounds::default());
        let parsed = parse_grant_args(&["I".into(), "CONSENT".into()]).unwrap();
        assert_eq!(parsed.phrase, "I CONSENT");
    }

    #[test]
    fn link_denials_carry_stable_codes() {
        use crate::commands::LinkDeny;
        assert_eq!(LinkDeny::Unlinked.code(), "unlinked_chat");
        assert_eq!(LinkDeny::WrongUser.code(), "chat_user_mismatch");
        assert_eq!(LinkDeny::TradingDisabled.code(), "trading_disabled");
        for deny in [
            LinkDeny::Unlinked,
            LinkDeny::WrongUser,
            LinkDeny::TradingDisabled,
        ] {
            assert!(!deny.explain().is_empty());
        }
    }
}
