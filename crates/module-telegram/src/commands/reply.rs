//! The **only** owner of user-facing text for the consent/trading commands.
//!
//! Every reply is built as *plain* text here and rendered for Telegram by
//! [`escape_markdown_v2`], which escapes the **whole** string. Escaping
//! everything (rather than hand-escaping dynamic fragments) is intentional:
//!
//! * it cannot be forgotten for a new field;
//! * mints, nonces and operator input cannot break the message;
//! * Telegram still renders the text correctly (escapes are invisible).
//!
//! [`Reply::plain`] returns the unescaped text, which is what the tests and the
//! audit trail assert on; the dispatcher sends [`Reply::markdown_v2`].
//!
//! Nothing here logs. No template ever contains a secret: the only dynamic
//! values are public chain data (mint), lamport/basis-point amounts, machine
//! reason text, and the one-time nonce that the reply's whole purpose is to
//! deliver.

use crate::commands::parse::LAMPORTS_PER_SOL;
use crate::trade_port::TradeAction;

/// The parse mode every consent/trading reply is sent with.
pub const PARSE_MODE: &str = "MarkdownV2";

/// Characters MarkdownV2 requires to be escaped.
const MD_V2_SPECIALS: &[char] = &[
    '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!', '\\',
];

/// Escape a plain string for `parse_mode = MarkdownV2`.
pub fn escape_markdown_v2(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    for ch in input.chars() {
        if MD_V2_SPECIALS.contains(&ch) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// `true` when every special character in `text` is already backslash-escaped.
///
/// Used by the tests as a structural check on every template.
pub fn is_markdown_v2_safe(text: &str) -> bool {
    let mut escaped = false;
    for ch in text.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if MD_V2_SPECIALS.contains(&ch) {
            return false;
        }
    }
    !escaped
}

/// Render integer lamports as an exact SOL decimal (`50000000` → `0.05`).
pub fn format_sol(lamports: u64) -> String {
    let whole = lamports / LAMPORTS_PER_SOL;
    let fraction = lamports % LAMPORTS_PER_SOL;
    if fraction == 0 {
        return whole.to_string();
    }
    let mut digits = format!("{fraction:09}");
    while digits.ends_with('0') {
        digits.pop();
    }
    format!("{whole}.{digits}")
}

/// Render basis points as an exact percentage (`1250` → `12.5`).
pub fn format_percent(bps: u16) -> String {
    let whole = bps / 100;
    let fraction = bps % 100;
    if fraction == 0 {
        return whole.to_string();
    }
    let mut digits = format!("{fraction:02}");
    while digits.ends_with('0') {
        digits.pop();
    }
    format!("{whole}.{digits}")
}

/// Render basis points as a percentage with a `%` sign.
pub fn format_percent_signed(bps: u16) -> String {
    format!("{}%", format_percent(bps))
}

/// A reply: plain text plus its MarkdownV2 rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    plain: String,
}

impl Reply {
    /// Build a reply from plain text.
    pub fn new(plain: impl Into<String>) -> Self {
        Self {
            plain: plain.into(),
        }
    }

    /// The unescaped text (tests, audit assertions).
    pub fn plain(&self) -> &str {
        &self.plain
    }

    /// The escaped text actually sent to Telegram.
    pub fn markdown_v2(&self) -> String {
        escape_markdown_v2(&self.plain)
    }
}

/// The reply shown when no trading console is attached to the bot.
pub fn not_configured() -> Reply {
    Reply::new("Telegram trading is not configured on this deployment.")
}

/// "A pending command was created" — carries the one-time nonce.
#[allow(clippy::too_many_arguments)]
pub fn pending_created(
    action: TradeAction,
    mint: &str,
    amount_lamports: Option<u64>,
    percent_bps: Option<u16>,
    slippage_bps: u16,
    nonce: &str,
    ttl_secs: i64,
    mode: &str,
) -> Reply {
    let size = match action {
        TradeAction::Buy => format!("amount: {} SOL", format_sol(amount_lamports.unwrap_or(0))),
        TradeAction::Sell => format!(
            "amount: {} of the position",
            format_percent_signed(percent_bps.unwrap_or(0))
        ),
    };
    Reply::new(format!(
        "Pending {action} request (mode: {mode})\nmint: {mint}\n{size}\nslippage: {slip} bps\n\nConfirm with:\n/confirm {nonce}\n\nThis nonce is single-use and expires in {ttl} seconds. Use /cancel {nonce} to drop it. Nothing is executed until you confirm.",
        action = action.as_str().to_ascii_uppercase(),
        mode = mode,
        mint = mint,
        size = size,
        slip = slippage_bps,
        nonce = nonce,
        ttl = ttl_secs,
    ))
}

/// The order was handed to the port and accepted.
pub fn executed(action: TradeAction, reference: &str, detail: &str) -> Reply {
    Reply::new(format!(
        "Executed {action} — {detail} (ref: {reference}).",
        action = action.as_str().to_ascii_uppercase(),
        detail = detail,
        reference = reference
    ))
}

/// The port refused or failed the order.
pub fn execution_failed(action: TradeAction, code: &str, detail: &str) -> Reply {
    Reply::new(format!(
        "The {action} was NOT executed. Reason: {code} — {detail}. The command is now failed; no retry will happen automatically.",
        action = action.as_str(),
        code = code,
        detail = detail
    ))
}

/// A confirmation was accepted but the port had already run this command.
pub fn already_executed(action: TradeAction) -> Reply {
    Reply::new(format!(
        "This {action} command was already executed. Nothing else happened.",
        action = action.as_str()
    ))
}

/// A second `/confirm` arrived while the first is still executing.
pub fn confirm_in_flight(action: TradeAction) -> Reply {
    Reply::new(format!(
        "This {action} command is already confirmed and being executed. \
         The nonce is single-use, so nothing is sent twice.",
        action = action.as_str()
    ))
}

/// The command is in a non-actionable state.
pub fn already_resolved(status: &str) -> Reply {
    Reply::new(format!(
        "This command is already {status}. Nothing else happened.",
        status = status
    ))
}

/// The confirmation window elapsed.
pub fn expired(ttl_secs: i64) -> Reply {
    Reply::new(format!(
        "This confirmation expired after {ttl_secs} seconds. Send the command again to get a fresh nonce.",
        ttl_secs = ttl_secs
    ))
}

/// The requested command was cancelled.
pub fn cancelled() -> Reply {
    Reply::new("Cancelled. The nonce can no longer be used.")
}

/// A generic refusal with a machine code and a human explanation.
pub fn denied(code: &str, explanation: &str) -> Reply {
    Reply::new(format!("Refused ({code}): {explanation}"))
}

/// A refusal that also tells the operator what to do next.
pub fn denied_with_hint(code: &str, explanation: &str, hint: &str) -> Reply {
    Reply::new(format!(
        "Refused ({code}): {explanation}\n{hint}",
        code = code,
        explanation = explanation,
        hint = hint
    ))
}

/// `/consent grant` succeeded.
pub fn consent_granted(
    organization_id: &str,
    max_order_lamports: u64,
    daily_cap_lamports: u64,
    expires_at: &str,
    minutes_remaining: i64,
) -> Reply {
    Reply::new(format!(
        "Consent granted for {org}.\nper-order cap: {order} SOL\n24h cap: {daily} SOL\nexpires: {expires} (in {minutes} minutes)\n\nTrading stays paper-only unless the operator opens both live gates. Review with /consent status and stop everything with /consent revoke.",
        org = organization_id,
        order = format_sol(max_order_lamports),
        daily = format_sol(daily_cap_lamports),
        expires = expires_at,
        minutes = minutes_remaining
    ))
}

/// `/consent status` with no consent on file.
pub fn consent_missing() -> Reply {
    Reply::new(
        "No consent is on file for this chat. Grant one with:\n/consent grant [max_order_sol] [daily_cap_sol] [ttl_hours] I CONSENT TO TELEGRAM TRADING",
    )
}

/// `/consent status` for an existing consent.
#[allow(clippy::too_many_arguments)]
pub fn consent_status(
    status: &str,
    organization_id: &str,
    scope: &str,
    max_order_lamports: u64,
    daily_cap_lamports: u64,
    seconds_remaining: i64,
    revoked: bool,
) -> Reply {
    let lifecycle = if revoked {
        format!("status: {status}", status = status)
    } else {
        format!(
            "status: {status}\ntime left: {minutes} minutes",
            status = status,
            minutes = seconds_remaining / 60
        )
    };
    Reply::new(format!(
        "Consent for {org}\n{lifecycle}\nscope: {scope}\nper-order cap: {order} SOL\n24h cap: {daily} SOL",
        org = organization_id,
        lifecycle = lifecycle,
        scope = scope,
        order = format_sol(max_order_lamports),
        daily = format_sol(daily_cap_lamports)
    ))
}

/// `/consent revoke` completed, cancelling `cancelled` pending commands.
pub fn consent_revoked(cancelled: u64) -> Reply {
    Reply::new(format!(
        "Consent revoked. {cancelled} pending command(s) were cancelled. Trading commands are refused until a new consent is granted.",
        cancelled = cancelled
    ))
}

/// `/consent revoke` with nothing to revoke.
pub fn consent_revoke_nothing() -> Reply {
    Reply::new("There is no live consent to revoke for this chat.")
}

/// `/pending` with a list of open commands.
pub fn pending_list(lines: &[String]) -> Reply {
    let mut out = String::from("Pending commands for this chat:\n");
    for line in lines {
        out.push_str("• ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str(
        "\nNonces are never stored, so a command can only be confirmed with the nonce from its original reply. Cancel one with /cancel <nonce>.",
    );
    Reply::new(out.trim_end().to_string())
}

/// `/pending` with nothing open.
pub fn pending_empty() -> Reply {
    Reply::new("No pending commands for this chat.")
}

/// The command reference shown by `/consent help` and by `/help`.
pub fn trade_help() -> Reply {
    Reply::new(
        [
            "Telegram trading commands",
            "/consent grant [max_order_sol] [daily_cap_sol] [ttl_hours] <phrase> — grant trading consent (owner only)",
            "/consent status — show the consent on file",
            "/consent revoke — revoke immediately and cancel pending commands",
            "/buy <mint> <amount_sol> [slippage_bps] — request a buy",
            "/sell <mint> <percent|all> [slippage_bps] — request a sell",
            "/confirm <nonce> — execute a pending command",
            "/cancel <nonce> — cancel a pending command",
            "/pending — list pending commands for this chat",
        ]
        .join("\n"),
    )
}

/// `/pending` line for one command.
pub fn pending_line(
    action: TradeAction,
    mint: &str,
    amount_lamports: Option<u64>,
    percent_bps: Option<u16>,
    seconds_left: i64,
    command_id: &str,
) -> String {
    let size = match action {
        TradeAction::Buy => format!("{} SOL", format_sol(amount_lamports.unwrap_or(0))),
        TradeAction::Sell => format_percent_signed(percent_bps.unwrap_or(0)),
    };
    format!(
        "{action} {size} of {mint} — {ttl}s left (id {id})",
        action = action.as_str().to_ascii_uppercase(),
        size = size,
        mint = mint,
        ttl = seconds_left,
        id = command_id
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping_covers_every_special() {
        let raw = "_*[]()~`>#+-=|{}.!\\";
        let escaped = escape_markdown_v2(raw);
        assert_eq!(
            escaped,
            "\\_\\*\\[\\]\\(\\)\\~\\`\\>\\#\\+\\-\\=\\|\\{\\}\\.\\!\\\\"
        );
        assert!(is_markdown_v2_safe(&escaped));
        assert!(!is_markdown_v2_safe(raw));
        assert_eq!(
            escape_markdown_v2("plain words /confirm abc123"),
            "plain words /confirm abc123"
        );
        // A trailing lone backslash is not safe.
        assert!(!is_markdown_v2_safe("abc\\"));
    }

    #[test]
    fn sol_formatting_is_exact() {
        let table = [
            (0u64, "0"),
            (1, "0.000000001"),
            (50_000_000, "0.05"),
            (100_000_000, "0.1"),
            (1_000_000_000, "1"),
            (1_500_000_000, "1.5"),
            (200_000_000, "0.2"),
            (18_446_744_073_709_551_615, "18446744073.709551615"),
        ];
        for (lamports, expected) in table {
            assert_eq!(format_sol(lamports), expected, "{lamports}");
        }
    }

    #[test]
    fn percent_formatting_is_exact() {
        let table = [
            (10_000u16, "100"),
            (5_000, "50"),
            (1_250, "12.5"),
            (50, "0.5"),
            (1, "0.01"),
            (3_333, "33.33"),
        ];
        for (bps, expected) in table {
            assert_eq!(format_percent(bps), expected, "{bps}");
            assert_eq!(format_percent_signed(bps), format!("{expected}%"));
        }
    }

    #[test]
    fn every_template_renders_as_safe_markdown() {
        let replies = [
            not_configured(),
            pending_created(
                TradeAction::Buy,
                "So11111111111111111111111111111111111111112",
                Some(50_000_000),
                None,
                100,
                "0123456789abcdef0123456789abcdef",
                crate::commands::pending::COMMAND_TTL_SECS,
                "paper",
            ),
            pending_created(
                TradeAction::Sell,
                "So11111111111111111111111111111111111111112",
                None,
                Some(10_000),
                100,
                "0123456789abcdef0123456789abcdef",
                60,
                "LIVE",
            ),
            executed(TradeAction::Buy, "sig-1", "filled 0.05 SOL (simulated)"),
            execution_failed(TradeAction::Buy, "risk_rejected", "max exposure"),
            already_executed(TradeAction::Sell),
            already_resolved("executed"),
            expired(60),
            cancelled(),
            denied("no_consent", "no active consent for this chat"),
            denied_with_hint("no_consent", "none", "grant one with /consent grant"),
            consent_granted(
                "org-1",
                50_000_000,
                200_000_000,
                "2026-01-01 00:00 UTC",
                1_440,
            ),
            consent_missing(),
            consent_status(
                "active",
                "org-1",
                "buy, sell",
                50_000_000,
                200_000_000,
                600,
                false,
            ),
            consent_status(
                "revoked",
                "org-1",
                "buy, sell",
                50_000_000,
                200_000_000,
                0,
                true,
            ),
            consent_revoked(2),
            consent_revoke_nothing(),
            pending_list(&[pending_line(
                TradeAction::Buy,
                "So11111111111111111111111111111111111111112",
                Some(50_000_000),
                None,
                42,
                "abcd-1234",
            )]),
            pending_empty(),
            trade_help(),
        ];
        for reply in replies {
            let rendered = reply.markdown_v2();
            assert!(
                is_markdown_v2_safe(&rendered),
                "unsafe markdown: {rendered}"
            );
            assert!(!reply.plain().is_empty());
        }
    }

    #[test]
    fn pending_reply_carries_the_nonce_and_the_ttl() {
        let reply = pending_created(
            TradeAction::Buy,
            "So11111111111111111111111111111111111111112",
            Some(50_000_000),
            None,
            100,
            "0123456789abcdef0123456789abcdef",
            60,
            "paper",
        );
        assert!(reply
            .plain()
            .contains("/confirm 0123456789abcdef0123456789abcdef"));
        assert!(reply.plain().contains("0.05 SOL"));
        assert!(reply.plain().contains("expires in 60 seconds"));
        assert!(reply.plain().contains("Pending BUY"));
        let sell = pending_created(
            TradeAction::Sell,
            "So11111111111111111111111111111111111111112",
            None,
            Some(5_000),
            250,
            "abc",
            60,
            "paper",
        );
        assert!(sell.plain().contains("50% of the position"));
        assert!(sell.plain().contains("250 bps"));
    }

    #[test]
    fn help_mentions_every_command() {
        let help = trade_help();
        for command in [
            "/consent grant",
            "/consent status",
            "/consent revoke",
            "/buy",
            "/sell",
            "/confirm",
            "/cancel",
            "/pending",
        ] {
            assert!(help.plain().contains(command), "missing {command}");
        }
    }

    #[test]
    fn parse_mode_is_markdown_v2() {
        assert_eq!(PARSE_MODE, "MarkdownV2");
    }
}
