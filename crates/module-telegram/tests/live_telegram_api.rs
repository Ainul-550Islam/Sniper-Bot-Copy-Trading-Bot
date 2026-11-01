//! LIVE Telegram Bot API probe — a real HTTPS call to `api.telegram.org`, with
//! the error semantics that a deployment depends on.
//!
//! `#[ignore]`d by default: it needs the public internet. Run it with
//!
//! ```text
//! cargo test -p module-telegram --test live_telegram_api -- --ignored --nocapture --test-threads=1
//! ```
//!
//! What this test can and cannot prove, stated plainly:
//!
//! * CAN: the client speaks the real Bot API endpoint, the real `401
//!   Unauthorized` path is surfaced as a typed error, and the bot token never
//!   appears in the error text (the Bot API puts the token in the URL path, so
//!   a leaked `reqwest` error would leak the secret).
//! * CANNOT: send a message, read updates, or exercise commands — those need a
//!   real bot token from @BotFather, which this environment does not have and
//!   will not fake with a stub server.
//!
//! If `TELEGRAM_LIVE_TOKEN` (a real bot token) is exported, the same file also
//! verifies `getMe`-level connectivity via `getUpdates` returning `ok: true`.

use module_telegram::api::TelegramApi;

/// Shaped like a real token (`<bot_id>:<35 chars>`) but deliberately invalid,
/// so the Bot API answers exactly as it would for a revoked token.
const PLACEHOLDER_TOKEN: &str = "123456789:AAFakeTokenThatMustNeverBeLoggedByTheClient";

#[tokio::test]
#[ignore = "requires the public internet (api.telegram.org)"]
async fn live_telegram_rejects_an_invalid_token_and_never_leaks_it() {
    let api = TelegramApi::new(PLACEHOLDER_TOKEN).expect("client builds from a non-empty token");
    // timeout_secs = 0 keeps the live probe fast; nothing is long-polled.
    let err = api
        .get_updates(0, 0)
        .await
        .expect_err("the real Bot API must reject a placeholder token");
    let text = err.to_string();

    assert!(
        text.contains("getUpdates not ok"),
        "unexpected live error text: {text}"
    );
    assert!(
        !text.contains(PLACEHOLDER_TOKEN),
        "the bot token leaked into an error string: {text}"
    );
    assert!(
        !text.contains("AAFakeToken"),
        "part of the bot token leaked into an error string: {text}"
    );
    eprintln!("LIVE telegram 401 path: {text}");
}

#[tokio::test]
#[ignore = "requires the public internet (api.telegram.org)"]
async fn live_telegram_send_message_fails_closed_and_redacted() {
    let api = TelegramApi::new(PLACEHOLDER_TOKEN).expect("client");
    let err = api
        .send_message(1, "sniper-suite live probe", None)
        .await
        .expect_err("the real Bot API must reject a placeholder token");
    let text = err.to_string();
    assert!(
        !text.contains(PLACEHOLDER_TOKEN) && !text.contains("AAFakeToken"),
        "the bot token leaked into an error string: {text}"
    );
    eprintln!("LIVE telegram sendMessage path: {text}");
}

#[tokio::test]
#[ignore = "requires the public internet + a real TELEGRAM_LIVE_TOKEN"]
async fn live_telegram_real_bot_token_round_trip() {
    let Ok(token) = std::env::var("TELEGRAM_LIVE_TOKEN") else {
        eprintln!(
            "SKIPPED: set TELEGRAM_LIVE_TOKEN (a real bot token) for the authenticated round trip"
        );
        return;
    };
    if token.trim().is_empty() {
        eprintln!("SKIPPED: TELEGRAM_LIVE_TOKEN is empty");
        return;
    }
    let api = TelegramApi::new(token).expect("client");
    // A short poll with offset 0: `ok: true` with an (empty) update list proves
    // the token is accepted and the Bot API replies in the shape we parse.
    let updates = api
        .get_updates(0, 0)
        .await
        .expect("a real bot token must be accepted by the Bot API");
    eprintln!(
        "LIVE telegram authenticated getUpdates ok, updates_pending={}",
        updates.len()
    );
}
